"""Protocol integration tests, without credentials or model requests."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import unittest
import tempfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('client', Path(__file__).with_name('codex_app_server.py'))
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)

SERVER = '''
import json, sys
for line in sys.stdin:
 v=json.loads(line); m=v['method']; p=v.get('params', {})
 if m=='initialized': continue
 if m=='initialize': r={}
 elif m=='thread/resume' and p['threadId'] in ('locked','invalid'):
  print(json.dumps({'id':v['id'],'error':{'code':-32600,'message':'thread locked already has an active writer' if p['threadId']=='locked' else 'invalid thread'}}),flush=True)
  continue
 elif m=='thread/fork':
  assert p['threadId']=='locked' and p['excludeTurns'] is True
  r={'thread':{'id':'branched'}}
 elif m in ('thread/start','thread/resume'): r={'thread':{'id':p.get('threadId','new')}}
 elif m=='turn/start': r={'turn':{'id':'turn'}}
 print(json.dumps({'id':v['id'],'result':r}),flush=True)
 if m=='turn/start':
  mode=p['input'][0]['text']
  if mode=='eof': sys.exit(0)
  if mode=='approval':
   print(json.dumps({'id':99,'method':'item/commandExecution/requestApproval','params':{}}),flush=True)
   continue
  for method,params in [('item/agentMessage/delta',{'itemId':'msg','delta':'Hello'}),('item/completed',{'item':{'id':'msg','type':'agentMessage','text':'Hello'}}),('turn/completed',{'turn':{'status':mode}})]:
   print(json.dumps({'method':method,'params':params}),flush=True)
'''

class ProtocolTests(unittest.TestCase):
    def run_turn(self, mode='completed', thread=None):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            client.run({'command': [sys.executable, '-u', '-c', SERVER],
                        'thread': thread or {}}, mode)
        return [json.loads(line) for line in output.getvalue().splitlines()]

    def test_start_stream_complete(self):
        events = self.run_turn()
        self.assertEqual(events[0]['thread_id'], 'new')
        self.assertEqual(events[1]['item']['text'], 'Hello')
        self.assertEqual(events[-1]['type'], 'turn.completed')

    def test_resume(self):
        self.assertEqual(self.run_turn(thread={'threadId': 'saved'})[0]['thread_id'], 'saved')

    def test_locked_thread_branches_before_sending_message(self):
        events = self.run_turn(thread={'threadId': 'locked'})
        started = [e for e in events if e['type'] == 'thread.started']
        self.assertEqual(started[0]['thread_id'], 'branched')
        self.assertEqual(events[-1]['type'], 'turn.completed')

    def test_other_resume_errors_do_not_branch(self):
        with self.assertRaises(client.RpcError):
            self.run_turn(thread={'threadId': 'invalid'})

    def test_private_import_is_a_snapshot_and_preserves_existing_history(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, home = root / 'desktop', root / 'private'
            (source / 'sessions').mkdir(parents=True)
            home.mkdir()
            thread = '00000000-0000-4000-8000-000000000001'
            rollout = source / 'sessions' / ('rollout-' + thread + '.jsonl')
            rollout.write_bytes(b'{"type":"session_meta"}\npartial')
            (home / 'import-home.txt').write_text(str(source))
            with patch.dict('os.environ', {'CODEX_HOME': str(home)}):
                command = client.prepare_session(['codex', 'app-server'], {'threadId': thread})
                target = home / 'sessions' / rollout.name
                self.assertEqual(target.read_bytes(), b'{"type":"session_meta"}\n')
                self.assertIn('sqlite_home=' + json.dumps(str(home)), command)
                target.write_bytes(b'private history\n')
                client.prepare_session(['codex', 'app-server'], {'threadId': thread})
                self.assertEqual(target.read_bytes(), b'private history\n')
                self.assertTrue(rollout.read_bytes().endswith(b'partial'))

    def test_failures(self):
        for mode in ('failed', 'interrupted', 'eof', 'approval'):
            with self.subTest(mode=mode), self.assertRaises(RuntimeError):
                self.run_turn(mode)

if __name__ == '__main__':
    unittest.main()
