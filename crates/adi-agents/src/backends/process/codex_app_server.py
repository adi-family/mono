"""Bounded stdio app-server client; stdout retains ADI's historical event log format."""
import json
import os
from pathlib import Path
import uuid
import subprocess
import sys


class RpcError(RuntimeError):
    def __init__(self, error):
        self.error = error
        super().__init__(json.dumps(error))


def emit(kind, **fields):
    print(json.dumps(dict(type=kind, **fields)), flush=True)


def prepare_session(command, params):
    home_value = os.environ.get('CODEX_HOME')
    if not home_value:
        return command
    home = Path(home_value)
    marker = home / 'import-home.txt'
    if not marker.is_file():
        return command
    # Pin SQLite too: a workspace config must not redirect state back to desktop.
    command = [*command, '-c', 'sqlite_home=' + json.dumps(str(home)),
               '-c', 'cli_auth_credentials_store="file"']
    thread_id = params.get('threadId')
    if not thread_id:
        return command
    thread_id = str(uuid.UUID(thread_id))
    if any((home / 'sessions').rglob('*' + thread_id + '.jsonl')):
        return command
    source = Path(marker.read_text())
    candidates = list((source / 'sessions').rglob('*' + thread_id + '.jsonl'))
    if len(candidates) != 1:
        raise RuntimeError('Cannot import the existing Codex conversation into this backend home: rollout missing or ambiguous')
    path = candidates[0]
    # Snapshot only complete records; never share locks, database files or writable rollouts.
    data = path.read_bytes()
    data = data[:data.rfind(b'\n') + 1]
    if not data:
        raise RuntimeError('Cannot import an empty Codex conversation')
    target = home / 'sessions' / path.relative_to(source / 'sessions')
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_suffix('.' + str(uuid.uuid4()) + '.tmp')
    temporary.write_bytes(data)
    temporary.chmod(0o600)
    # Exclusive publication preserves an import another turn may already have completed.
    try:
        os.link(temporary, target)
    except FileExistsError:
        pass
    finally:
        temporary.unlink()
    return command


def run(options, message):
    command = prepare_session(options['command'], options['thread'])
    child = subprocess.Popen(command, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, bufsize=1)
    serial = 0
    usage = {}
    items = {}
    completed = False

    def send(value):
        child.stdin.write(json.dumps(value) + '\n')
        child.stdin.flush()

    def receive():
        line = child.stdout.readline()
        if not line:
            raise RuntimeError('Codex app-server closed before completing the turn')
        return json.loads(line)

    def event(value):
        nonlocal usage, completed
        method = value.get('method', '')
        p = value.get('params', {})
        if 'id' in value and method:
            # There is no interactive approval UI in the detached runner. Fail explicitly
            # instead of silently approving or leaving an unanswered request hanging.
            send({'id': value['id'], 'error': {'code': -32601,
                  'message': 'ADI detached runner cannot handle interactive requests'}})
            raise RuntimeError('Codex requires interactive input: ' + method)
        if method in ('item/started', 'item/completed'):
            item = dict(p['item'])
            names = {'agentMessage': 'agent_message', 'commandExecution': 'command_execution',
                     'fileChange': 'file_change', 'mcpToolCall': 'mcp_tool_call',
                     'webSearch': 'web_search'}
            item['type'] = names.get(item['type'], item['type'])
            if item.get('status') == 'inProgress':
                item['status'] = 'in_progress'
            for source, target in [('aggregatedOutput', 'aggregated_output'), ('exitCode', 'exit_code')]:
                if source in item:
                    item[target] = item[source]
            if item['type'] == 'reasoning':
                item['text'] = '\n'.join(item.get('summary', []) + item.get('content', []))
            items[item['id']] = item
            emit(method.replace('/', '.'), item=item)
        elif method == 'item/agentMessage/delta':
            item = items.setdefault(p['itemId'], {'id': p['itemId'], 'type': 'agent_message', 'text': ''})
            item['text'] = item.get('text', '') + p['delta']
            emit('item.updated', item=item)
        elif method == 'thread/tokenUsage/updated':
            u = p['tokenUsage']['last']
            usage = {'input_tokens': u.get('inputTokens', 0),
                     'cached_input_tokens': u.get('cachedInputTokens', 0),
                     'output_tokens': u.get('outputTokens', 0)}
        elif method == 'turn/completed':
            turn = p['turn']
            if turn['status'] != 'completed':
                raise RuntimeError(json.dumps(turn.get('error') or {'message': turn['status']}))
            emit('turn.completed', usage=usage)
            completed = True
            return True
        return False

    def request(method, params):
        nonlocal serial
        serial += 1
        send({'id': serial, 'method': method, 'params': params})
        while True:
            value = receive()
            if value.get('id') == serial and 'method' not in value:
                if 'error' in value:
                    raise RpcError(value['error'])
                return value['result']
            event(value)

    try:
        request('initialize', {'clientInfo': {'name': 'adi_family', 'title': 'ADI', 'version': '1.0.0'}})
        send({'method': 'initialized'})
        params = options['thread']
        method = 'thread/resume' if 'threadId' in params else 'thread/start'
        try:
            thread = request(method, params)['thread']['id']
        except RpcError as error:
            # Another client may keep an idle thread loaded for viewing. Preserve
            # its ownership and branch persisted history only for this conflict.
            if (method != 'thread/resume' or error.error.get('code') != -32600
                    or 'already has an active writer' not in error.error.get('message', '')):
                raise
            fork_params = dict(params, excludeTurns=True)
            thread = request('thread/fork', fork_params)['thread']['id']
            emit('item.completed', item={'id': 'adi-thread-fork', 'type': 'agent_message',
                 'text': 'ADI продолжает диалог в отдельной ветке с сохранённой историей: исходный диалог открыт в другом приложении.'})
        emit('thread.started', thread_id=thread)
        request('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': message}]})
        while not completed:
            event(receive())
    finally:
        child.stdin.close()
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        child.stdout.close()


if __name__ == '__main__':
    try:
        run(json.loads(sys.argv[1]), sys.argv[2])
    except Exception as error:
        emit('turn.failed', error={'message': str(error)})
        sys.exit(1)
