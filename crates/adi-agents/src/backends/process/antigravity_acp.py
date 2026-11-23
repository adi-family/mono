"""Bounded stdio Antigravity ACP client; stdout emits ADI's Antigravity stream-json NDJSON format."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time


class RpcError(RuntimeError):
    def __init__(self, error):
        self.error = error
        msg = error.get('message') if isinstance(error, dict) else None
        super().__init__(msg or json.dumps(error))


def emit(event, **fields):
    payload = {'event': event}
    payload.update(fields)
    print(json.dumps(payload), flush=True)


def _normalize_tool(name, args, title=''):
    if name == 'call_mcp_tool' and isinstance(args, dict):
        srv = args.get('ServerName') or args.get('serverName') or ''
        tn = args.get('ToolName') or args.get('toolName') or ''
        inner = args.get('Arguments')
        if inner is None:
            inner = args.get('arguments')
        if srv == 'adi' and tn:
            return tn, inner if inner is not None else {}
        if srv and tn:
            return f'mcp_{srv}_{tn}', inner if inner is not None else {}
    if isinstance(name, str) and name.startswith('mcp_adi_'):
        clean_args = args
        if isinstance(args, dict) and 'toolSummary' in args:
            clean_args = {k: v for k, v in args.items() if k != 'toolSummary'}
        return name[len('mcp_adi_'):], clean_args if clean_args is not None else {}
    return (name or title or 'tool'), (args if args is not None else {})


def _is_adi_tool(name, args, target_tool=None):
    if name == 'call_mcp_tool' and isinstance(args, dict):
        srv = args.get('ServerName') or args.get('serverName') or ''
        tn = args.get('ToolName') or args.get('toolName') or ''
        if srv == 'adi':
            return True if target_tool is None else (tn == target_tool)
        return False
    if isinstance(name, str) and name.startswith('mcp_adi_'):
        tn = name[len('mcp_adi_'):]
        return True if target_tool is None else (tn == target_tool)
    if target_tool is not None and name == target_tool:
        return True
    return False


def _is_declared_mcp(name, args, declared_servers):
    if name == 'call_mcp_tool' and isinstance(args, dict):
        srv = args.get('ServerName') or args.get('serverName') or ''
        return srv in declared_servers
    if isinstance(name, str):
        for srv in declared_servers:
            if name.startswith(f'mcp_{srv}_'):
                return True
    return False


def _should_allow_permission(tool_call, options, declared_servers):
    name = tool_call.get('name') or ''
    args = tool_call.get('args')
    if args is None:
        args = tool_call.get('rawInput') or {}
    kind = (tool_call.get('kind') or '').lower()

    code_only = options.get('code_only')
    if code_only:
        return _is_adi_tool(name, args, target_tool=code_only)

    if _is_declared_mcp(name, args, declared_servers):
        return True

    if options.get('dangerously_skip_permissions'):
        return True

    mode = (options.get('mode') or '').strip().lower()
    if mode in ('', 'auto'):
        return True
    if mode == 'plan':
        return kind in ('read', 'search', 'think')
    if mode == 'accept-edits':
        return kind in ('read', 'search', 'think', 'edit', 'fetch')
    return True


def _extract_output(update):
    for key in ('rawOutput', 'output', 'result'):
        raw = update.get(key)
        if raw is not None:
            if isinstance(raw, str):
                return raw
            return json.dumps(raw)
    content = update.get('content')
    if isinstance(content, list):
        parts = []
        for entry in content:
            if not isinstance(entry, dict):
                continue
            inner = entry.get('content') if isinstance(entry.get('content'), dict) else entry
            text = inner.get('text')
            if isinstance(text, str) and text:
                parts.append(text)
        if parts:
            return '\n'.join(parts)
    return ''


def _prepare_code_only_staging(staging_dir, code_only_tool, mcp_servers):
    root = Path(staging_dir)
    agents_dir = root / '.agents' / 'agents'
    agents_dir.mkdir(parents=True, exist_ok=True)
    agent_md = (
        '---\n'
        'name: adi-code-only\n'
        'description: ADI code-only execution agent\n'
        'mainAgent: true\n'
        'excludeDefaultComponents: true\n'
        'inheritMcp: true\n'
        '---\n'
        f'You are an ADI code-only agent. Your only execution tool is `{code_only_tool}` on MCP server `adi`.\n'
        f'If `mcp_adi_{code_only_tool}` is declared in your tools, call `mcp_adi_{code_only_tool}` directly with `{{"code": "<your code>"}}`.\n'
        f'Otherwise, invoke `call_mcp_tool` with `ServerName: "adi"`, `ToolName: "{code_only_tool}"`, '
        'and `Arguments: {"code": "<your code>"}`.\n'
        'Do not invoke any other tool (`list_resources`, `read_resource`, `manage_task`, or any other server or tool).\n'
    )
    (agents_dir / 'adi-code-only.md').write_text(agent_md, encoding='utf-8')

    adi_server = next((s for s in mcp_servers if s.get('name') == 'adi'), None)
    if adi_server is not None:
        cfg = {
            'mcpServers': {
                'adi': {
                    'command': adi_server['command'],
                    'args': list(adi_server.get('args', [])),
                }
            }
        }
        (root / '.agents' / 'mcp_config.json').write_text(json.dumps(cfg), encoding='utf-8')


def _pick_thought_level(requested, config_options):
    if not requested:
        return None
    allowed = []
    for opt in config_options or []:
        if opt.get('id') == 'thought_level':
            allowed = [o.get('value') for o in opt.get('options') or [] if o.get('value')]
            break
    if requested == 'max':
        if 'xhigh' in allowed:
            return 'xhigh'
        return 'high'
    if allowed and requested not in allowed:
        if requested == 'xhigh' and 'high' in allowed:
            return 'high'
        return None
    return requested


def run(options, message):
    started_at = time.monotonic()
    command = list(options['command'])
    mcp_servers = list(options.get('mcp_servers') or [])
    declared_servers = {s.get('name') for s in mcp_servers if s.get('name')}
    code_only = options.get('code_only')

    staging_ctx = tempfile.TemporaryDirectory(prefix='adi-agy-code-only-') if code_only else None
    if staging_ctx is not None:
        _prepare_code_only_staging(staging_ctx.name, code_only, mcp_servers)
        command.extend(['--add-dir', staging_ctx.name, '--agent', 'adi-code-only'])

    child = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    timed_out = False
    timer = None
    timeout_seconds = options.get('timeout_seconds')
    if isinstance(timeout_seconds, (int, float)) and timeout_seconds > 0:
        def _on_timeout():
            nonlocal timed_out
            timed_out = True
            try:
                child.terminate()
            except Exception:
                pass
        timer = threading.Timer(timeout_seconds, _on_timeout)
        timer.daemon = True
        timer.start()

    serial = 0
    sid = options.get('conversation_id') or ''
    next_step = 0
    current_msg_step = None
    current_thought_step = None
    current_msg_text = ''
    tool_steps = {}
    denied_actions = []
    tool_calls_count = 0

    def send(value):
        child.stdin.write(json.dumps(value) + '\n')
        child.stdin.flush()

    def receive():
        while True:
            line = child.stdout.readline()
            if not line:
                if timed_out:
                    raise RuntimeError(f'Antigravity ACP turn timed out after {timeout_seconds}s')
                raise RuntimeError('Antigravity ACP server closed before completing the turn')
            stripped = line.strip()
            if not stripped:
                continue
            try:
                return json.loads(stripped)
            except json.JSONDecodeError:
                continue

    def handle_permission(msg):
        params = msg.get('params') or {}
        tool_call = params.get('toolCall') or {}
        opts = params.get('options') or []
        allow = _should_allow_permission(tool_call, options, declared_servers)
        if allow:
            pick = next(
                (o['optionId'] for o in opts if o.get('kind') in ('allow_always', 'allow_once')),
                opts[0]['optionId'] if opts else 'allow_once',
            )
            outcome = {'outcome': 'selected', 'optionId': pick}
        else:
            name = tool_call.get('name') or ''
            args = tool_call.get('args')
            if args is None:
                args = tool_call.get('rawInput') or {}
            display, _ = _normalize_tool(name, args, tool_call.get('title') or '')
            denied_actions.append({
                'action': (tool_call.get('kind') or 'tool'),
                'display_name': display,
            })
            reject = next(
                (o['optionId'] for o in opts if o.get('kind') in ('reject_once', 'reject_always')),
                None,
            )
            if reject is not None:
                outcome = {'outcome': 'selected', 'optionId': reject}
            else:
                outcome = {'outcome': 'cancelled'}
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'outcome': outcome}})

    def handle_update(update):
        nonlocal next_step, current_msg_step, current_thought_step, current_msg_text, tool_calls_count
        kind = update.get('sessionUpdate')
        if kind == 'agent_message_chunk':
            text = (update.get('content') or {}).get('text') or ''
            if not text:
                return
            if current_msg_step is None:
                current_msg_step = next_step
                next_step += 1
                current_msg_text = ''
                current_thought_step = None
            current_msg_text += text
            emit(
                'step_update',
                step_update={
                    'conversation_id': sid,
                    'step_index': current_msg_step,
                    'state': 'ACTIVE',
                    'step_type': 'agent_response',
                    'text_delta': text,
                },
            )
        elif kind == 'agent_thought_chunk':
            text = (update.get('content') or {}).get('text') or ''
            if not text:
                return
            if current_thought_step is None:
                current_thought_step = next_step
                next_step += 1
                current_msg_step = None
            emit(
                'step_update',
                step_update={
                    'conversation_id': sid,
                    'step_index': current_thought_step,
                    'state': 'ACTIVE',
                    'step_type': 'thinking',
                    'text_delta': text,
                },
            )
        elif kind in ('tool_call', 'tool_call_update'):
            current_msg_step = None
            current_thought_step = None
            call_id = update.get('toolCallId') or f'tool-{next_step}'
            if call_id not in tool_steps:
                tool_steps[call_id] = next_step
                next_step += 1
                tool_calls_count += 1
            step_index = tool_steps[call_id]
            raw_args = update.get('args')
            if raw_args is None:
                raw_args = update.get('rawInput')
            display_name, parameters = _normalize_tool(
                update.get('name') or '',
                raw_args,
                update.get('title') or '',
            )
            status = (update.get('status') or '').lower()
            if status == 'completed':
                state = 'DONE'
            elif status in ('failed', 'cancelled', 'error'):
                state = 'ERROR'
            else:
                state = 'ACTIVE'
            tool_info = {
                'name': display_name,
                'parameters': parameters,
            }
            output = _extract_output(update)
            if state == 'ERROR':
                tool_info['error'] = {'message': output or 'tool call failed'}
            elif output:
                tool_info['output'] = output
            emit(
                'step_update',
                step_update={
                    'conversation_id': sid,
                    'step_index': step_index,
                    'state': state,
                    'step_type': 'tool',
                    'tool_name': display_name,
                    'tool_info': tool_info,
                },
            )

    def request(method, params, emit_updates=False):
        nonlocal serial
        serial += 1
        req_id = serial
        send({'jsonrpc': '2.0', 'id': req_id, 'method': method, 'params': params})
        while True:
            msg = receive()
            if 'id' in msg and msg.get('method'):
                if msg['method'] == 'session/request_permission':
                    handle_permission(msg)
                else:
                    send({
                        'jsonrpc': '2.0',
                        'id': msg['id'],
                        'error': {'code': -32601, 'message': 'Unsupported client request: ' + msg['method']},
                    })
                continue
            if msg.get('id') == req_id and 'method' not in msg:
                if 'error' in msg:
                    raise RpcError(msg['error'])
                return msg.get('result') or {}
            if emit_updates and msg.get('method') == 'session/update':
                update = (msg.get('params') or {}).get('update')
                if isinstance(update, dict):
                    handle_update(update)

    try:
        request('initialize', {'protocolVersion': 1, 'clientCapabilities': {}})

        session_params = {
            'cwd': options.get('cwd') or os.getcwd(),
            'mcpServers': mcp_servers,
        }
        system_prompt = (options.get('system_prompt') or '').strip()
        if system_prompt:
            session_params['_meta'] = {
                'antigravity': {
                    'systemPrompt': {'append': system_prompt},
                }
            }

        existing_id = (options.get('conversation_id') or '').strip()
        if existing_id:
            session_params['sessionId'] = existing_id
            session_res = request('session/load', session_params, emit_updates=False)
            sid = session_res.get('sessionId') or existing_id
        else:
            session_res = request('session/new', session_params, emit_updates=False)
            sid = session_res.get('sessionId') or ''
            if not sid:
                raise RuntimeError('Antigravity ACP session/new returned no sessionId')

        config_options = session_res.get('configOptions') or []

        model = (options.get('model') or '').strip()
        if model:
            model_res = request('session/set_model', {'sessionId': sid, 'modelId': model})
            if isinstance(model_res, dict) and model_res.get('configOptions'):
                config_options = model_res['configOptions']

        effort = _pick_thought_level((options.get('effort') or '').strip(), config_options)
        if effort:
            try:
                request('session/set_config_option', {
                    'sessionId': sid,
                    'configId': 'thought_level',
                    'value': effort,
                })
            except RpcError:
                pass

        mode = (options.get('mode') or '').strip()
        if mode and not code_only:
            try:
                request('session/set_mode', {'sessionId': sid, 'modeId': mode})
            except RpcError:
                pass

        init_info = {'cwd': session_params['cwd']}
        if model:
            init_info['model'] = model
        emit('init', conversation_id=sid, init=init_info)

        prompt_res = request(
            'session/prompt',
            {
                'sessionId': sid,
                'prompt': [{'type': 'text', 'text': message}],
            },
            emit_updates=True,
        )

        usage_raw = prompt_res.get('usage') or {}
        usage = {}
        for src, dst in [
            ('inputTokens', 'input_tokens'),
            ('outputTokens', 'output_tokens'),
            ('thoughtTokens', 'thinking_tokens'),
            ('cachedReadTokens', 'cache_read_tokens'),
            ('totalTokens', 'total_tokens'),
        ]:
            if src in usage_raw and usage_raw[src] is not None:
                usage[dst] = usage_raw[src]

        result = {
            'conversation_id': sid,
            'status': 'SUCCESS',
            'response': current_msg_text.strip(),
            'duration_seconds': round(time.monotonic() - started_at, 3),
            'num_turns': max(1, tool_calls_count + 1),
        }
        if usage:
            result['usage'] = usage
        if denied_actions:
            result['denied_actions'] = denied_actions
        emit('result', result=result)
    finally:
        if timer is not None:
            timer.cancel()
        if child.stdin is not None:
            try:
                child.stdin.close()
            except Exception:
                pass
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        if child.stdout is not None:
            try:
                child.stdout.close()
            except Exception:
                pass
        if staging_ctx is not None:
            staging_ctx.cleanup()


if __name__ == '__main__':
    try:
        run(json.loads(sys.argv[1]), sys.argv[2])
    except Exception as error:
        emit(
            'result',
            result={
                'conversation_id': '',
                'status': 'ERROR',
                'response': '',
                'error': str(error),
                'finished_reason': 'error',
            },
        )
        sys.exit(1)
