#!/usr/bin/env python3
"""Serve the real side-panel controller through an isolated native-host proxy.

Run on the configured SSH build host. This starts no browser or model. The
caller drives the fixture in an isolated remote Chrome session and stops this
server afterward. Agent CLI traffic goes only to tests/fixtures/panel_agent.py.
"""
import argparse
import atexit
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--work-dir', type=Path, required=True)
args = parser.parse_args()
root = args.work_dir.resolve()
root.mkdir(parents=True, exist_ok=True)
web = root / 'web'
shutil.copytree(args.source / 'extensions/ab-connect', web, dirs_exist_ok=True)
home = root / 'home'
bin_dir = home / '.local/bin'
bin_dir.mkdir(parents=True, exist_ok=True)
for agent in ['codex', 'claude', 'opencode', 'pi']:
    target = bin_dir / agent
    if not target.exists():
        target.symlink_to((args.source / 'cli/tests/fixtures/panel_agent.py').resolve())
fixture = args.source / 'cli/tests/fixtures/panel_agent.py'
profile = 'panel-ui-smoke'
env = os.environ.copy()
env.update(HOME=str(home), CODEX_HOME=str(home / '.codex'), CLAUDE_CONFIG_DIR=str(home / '.claude'),
           PI_CODING_AGENT_DIR=str(home / '.pi'), XDG_CONFIG_HOME=str(home / '.config'),
           PATH=str(bin_dir) + ':' + env.get('PATH', ''), CHROME_USE_RELAY_DIR=str(root / 'relay'),
           CHROME_USE_NO_UPDATE_CHECK='1', PANEL_FIXTURE_CAPTURE=str(root / 'protocol.jsonl'))
host = subprocess.Popen([str(args.binary), '__nm-host', 'chrome-extension://panel-ui-fixture/'],
                        env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
lock = threading.Lock()
sequence = 0


def write(value):
    data = json.dumps(value).encode()
    host.stdin.write(struct.pack('=I', len(data)) + data)
    host.stdin.flush()


def read_exact(size):
    data = bytearray()
    while len(data) < size:
        chunk = host.stdout.read(size - len(data))
        if not chunk:
            raise RuntimeError('Fixture native host exited')
        data.extend(chunk)
    return bytes(data)


def read():
    size = struct.unpack('=I', read_exact(4))[0]
    if size > 1024 * 1024:
        raise RuntimeError('Unbounded native frame')
    return json.loads(read_exact(size))


write({'method': 'hello', 'version': 'fixture', 'profileId': profile})
capabilities = None
while capabilities is None:
    message = read()
    capabilities = message.get('params', {}).get('agentPanel')
    if message.get('method') == 'ping':
        write({'method': 'pong'})


def request(operation, params):
    global sequence
    with lock:
        sequence += 1
        identity = f'fixture:{sequence}'
        write({'method': 'agentPanel', 'requestId': identity, 'operation': operation, 'params': params})
        while True:
            message = read()
            if message.get('method') == 'ping':
                write({'method': 'pong'})
            if message.get('method') == 'agentPanelReply' and message.get('requestId') == identity:
                with (root / 'operations.jsonl').open('a') as proof:
                    proof.write(json.dumps({'operation': operation, 'reply': message}) + '\n')
                return {'success': 'error' not in message, **({'error': message['error']} if 'error' in message else {'result': message['result']})}


# Only the extension API shim is synthetic. The controller, UI, host frames,
# conversation service, files, backend protocols and scheduler are real.
shim = '''<script>
window.chrome={runtime:{onMessage:{addListener(){}},sendMessage:async request=>{
 const reply=await fetch('/fixture-api',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(request)});return reply.json();
}},storage:{local:{get:async keys=>Object.fromEntries((Array.isArray(keys)?keys:[keys]).map(key=>[key,JSON.parse(sessionStorage.getItem(key)||'null')])),set:async values=>{for(const [key,value] of Object.entries(values))sessionStorage.setItem(key,JSON.stringify(value));}}},tabs:{query:async()=>[]}};
</script>'''
page = (web / 'sidepanel.html').read_text().replace('</head>', shim + '</head>')
(web / 'sidepanel.html').write_text(page)


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(web), **kw)

    def do_POST(self):
        if self.path != '/fixture-api':
            self.send_error(404)
            return
        size = int(self.headers.get('Content-Length', '0'))
        if size > 768 * 1024:
            self.send_error(413)
            return
        value = json.loads(self.rfile.read(size))
        if value.get('type') == 'ab-status':
            reply = {'connected': True, 'agentPanel': capabilities}
        elif value.get('type') == 'ab-agent-request':
            reply = request(value['operation'], value.get('params', {}))
        else:
            reply = {'success': False, 'error': 'Unknown fixture operation'}
        data = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def cleanup():
    try:
        reply = request('conversationList', {})
        for chat in reply.get('result', {}).get('conversations', []):
            request('conversationStop', {'conversationId': chat['id']})
        request('serviceStop', {})
    except (OSError, RuntimeError, BrokenPipeError):
        pass
    host.terminate()
    try:
        host.wait(timeout=5)
    except subprocess.TimeoutExpired:
        host.kill()
        host.wait()


atexit.register(cleanup)
signal.signal(signal.SIGTERM, lambda *_: exit(0))
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
(root / 'server.pid').write_text(str(os.getpid()))
(root / 'url').write_text(f'http://127.0.0.1:{server.server_port}/sidepanel.html')
print((root / 'url').read_text(), flush=True)
server.serve_forever()
