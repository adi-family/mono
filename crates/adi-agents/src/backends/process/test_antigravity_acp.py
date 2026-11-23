"""Protocol integration tests for antigravity_acp.py, without credentials or model requests."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import unittest

spec = importlib.util.spec_from_file_location('client', Path(__file__).with_name('antigravity_acp.py'))
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)

FAKE_ACP_SERVER = r'''
import json, os, sys

mode = None
seen_add_dirs = []
seen_agent = None
argv = sys.argv[1:]
i = 0
while i < len(argv):
    if argv[i] == '--add-dir' and i + 1 < len(argv):
        seen_add_dirs.append(argv[i + 1])
        i += 2
    elif argv[i] == '--agent' and i + 1 < len(argv):
        seen_agent = argv[i + 1]
        i += 2
    else:
        i += 1

def send(obj):
    print(json.dumps(obj), flush=True)

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    mid = msg.get('id')
    m = msg.get('method')
    p = msg.get('params') or {}
    if m == 'initialize':
        send({'jsonrpc': '2.0', 'id': mid, 'result': {'protocolVersion': 1}})
    elif m == 'session/new':
        send({
            'jsonrpc': '2.0',
            'id': mid,
            'result': {
                'sessionId': 'sess-new',
                'configOptions': [
                    {'id': 'thought_level', 'options': [{'value': 'low'}, {'value': 'medium'}, {'value': 'high'}]},
                ],
            },
        })
    elif m == 'session/load':
        # Replay a stale update from an earlier turn; the client must ignore it.
        send({
            'jsonrpc': '2.0',
            'method': 'session/update',
            'params': {'update': {'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': 'STALE_REPLAY'}}},
        })
        send({'jsonrpc': '2.0', 'id': mid, 'result': {'sessionId': p['sessionId']}})
    elif m in ('session/set_model', 'session/set_config_option', 'session/set_mode'):
        send({'jsonrpc': '2.0', 'id': mid, 'result': {}})
    elif m == 'session/prompt':
        text = p['prompt'][0]['text']
        if text == 'eof':
            sys.exit(0)
        if text == 'check_staging':
            assert seen_agent == 'adi-code-only', f'unexpected agent {seen_agent}'
            assert len(seen_add_dirs) == 1
            agent_md = os.path.join(seen_add_dirs[0], '.agents', 'agents', 'adi-code-only.md')
            mcp_cfg = os.path.join(seen_add_dirs[0], '.agents', 'mcp_config.json')
            assert os.path.isfile(agent_md)
            assert os.path.isfile(mcp_cfg)
            # Ask permission for both TsCode (must be approved) and run_command (must be rejected)
            send({
                'jsonrpc': '2.0',
                'id': 900,
                'method': 'session/request_permission',
                'params': {
                    'toolCall': {
                        'name': 'call_mcp_tool',
                        'kind': 'execute',
                        'args': {'ServerName': 'adi', 'ToolName': 'TsCode', 'Arguments': {'code': '1+1'}},
                    },
                    'options': [
                        {'optionId': 'allow_once', 'kind': 'allow_once'},
                        {'optionId': 'reject_once', 'kind': 'reject_once'},
                    ],
                },
            })
            reply_ok = json.loads(sys.stdin.readline())
            assert reply_ok['result']['outcome'] == {'outcome': 'selected', 'optionId': 'allow_once'}, reply_ok
            send({
                'jsonrpc': '2.0',
                'id': 901,
                'method': 'session/request_permission',
                'params': {
                    'toolCall': {'name': 'run_command', 'kind': 'execute', 'args': {'CommandLine': 'rm -rf /'}},
                    'options': [
                        {'optionId': 'allow_once', 'kind': 'allow_once'},
                        {'optionId': 'reject_once', 'kind': 'reject_once'},
                    ],
                },
            })
            reply_no = json.loads(sys.stdin.readline())
            assert reply_no['result']['outcome'] == {'outcome': 'selected', 'optionId': 'reject_once'}, reply_no
            send({
                'jsonrpc': '2.0',
                'method': 'session/update',
                'params': {'update': {
                    'sessionUpdate': 'tool_call',
                    'toolCallId': 'call-1',
                    'name': 'call_mcp_tool',
                    'args': {'ServerName': 'adi', 'ToolName': 'TsCode', 'Arguments': {'code': '1+1'}},
                    'status': 'in_progress',
                }},
            })
            send({
                'jsonrpc': '2.0',
                'method': 'session/update',
                'params': {'update': {
                    'sessionUpdate': 'tool_call_update',
                    'toolCallId': 'call-1',
                    'name': 'call_mcp_tool',
                    'args': {'ServerName': 'adi', 'ToolName': 'TsCode', 'Arguments': {'code': '1+1'}},
                    'status': 'completed',
                    'rawOutput': '2',
                }},
            })
            send({
                'jsonrpc': '2.0',
                'method': 'session/update',
                'params': {'update': {'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': 'Result is 2'}}},
            })
            send({'jsonrpc': '2.0', 'id': mid, 'result': {'stopReason': 'end_turn', 'usage': {'inputTokens': 50, 'outputTokens': 10}}})
            continue
        send({
            'jsonrpc': '2.0',
            'method': 'session/update',
            'params': {'update': {'sessionUpdate': 'agent_thought_chunk', 'content': {'type': 'text', 'text': 'thinking...'}}},
        })
        send({
            'jsonrpc': '2.0',
            'method': 'session/update',
            'params': {'update': {'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': 'Hello '}}},
        })
        send({
            'jsonrpc': '2.0',
            'method': 'session/update',
            'params': {'update': {'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': 'world'}}},
        })
        send({'jsonrpc': '2.0', 'id': mid, 'result': {'stopReason': 'end_turn', 'usage': {'inputTokens': 20, 'outputTokens': 5, 'thoughtTokens': 3}}})
'''


class ProtocolTests(unittest.TestCase):
    def run_turn(self, message='hi', **extra):
        opts = {
            'command': [sys.executable, '-u', '-c', FAKE_ACP_SERVER],
            'cwd': '/tmp',
            'mcp_servers': [{'name': 'adi', 'command': 'adi-mono', 'args': ['mcp-serve'], 'env': []}],
        }
        opts.update(extra)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            client.run(opts, message)
        return [json.loads(line) for line in output.getvalue().splitlines()]

    def test_first_turn_streams_thinking_text_and_usage(self):
        events = self.run_turn('hi', model='gemini-3.8-flash-low', effort='max', mode='auto')
        self.assertEqual(events[0]['event'], 'init')
        self.assertEqual(events[0]['conversation_id'], 'sess-new')
        thoughts = [e for e in events if e['event'] == 'step_update' and e['step_update']['step_type'] == 'thinking']
        self.assertEqual(len(thoughts), 1)
        self.assertEqual(thoughts[0]['step_update']['text_delta'], 'thinking...')
        result = events[-1]['result']
        self.assertEqual(result['status'], 'SUCCESS')
        self.assertEqual(result['response'], 'Hello world')
        self.assertEqual(result['usage']['input_tokens'], 20)
        self.assertEqual(result['usage']['output_tokens'], 5)
        self.assertEqual(result['usage']['thinking_tokens'], 3)

    def test_resume_ignores_replayed_updates_from_session_load(self):
        events = self.run_turn('hi', conversation_id='sess-old')
        self.assertEqual(events[0]['conversation_id'], 'sess-old')
        raw = json.dumps(events)
        self.assertNotIn('STALE_REPLAY', raw)
        self.assertEqual(events[-1]['result']['response'], 'Hello world')

    def test_code_only_stages_agent_allows_interpreter_and_rejects_other_tools(self):
        events = self.run_turn('check_staging', code_only='TsCode')
        tools = [e['step_update'] for e in events if e['event'] == 'step_update' and e['step_update']['step_type'] == 'tool']
        self.assertEqual(tools[-1]['tool_name'], 'TsCode')
        self.assertEqual(tools[-1]['tool_info']['parameters'], {'code': '1+1'})
        self.assertEqual(tools[-1]['tool_info']['output'], '2')
        result = events[-1]['result']
        self.assertEqual(result['response'], 'Result is 2')
        self.assertEqual(result['denied_actions'], [{'action': 'execute', 'display_name': 'run_command'}])

    def test_eof_raises_runtime_error(self):
        with self.assertRaises(RuntimeError):
            self.run_turn('eof')


if __name__ == '__main__':
    unittest.main()
