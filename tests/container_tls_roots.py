# Linux Docker/OpenSSL integration regression; run with an image tag argument.
# All TLS traffic stays on loopback; no real Discord request is made.
import hashlib
import hmac
import http.server
import json
import os
import pathlib
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

image = sys.argv[1]
fixture = tempfile.TemporaryDirectory(prefix='relay-tls-roots-', dir=os.environ.get('TMPDIR', pathlib.Path.home()))
base = pathlib.Path(fixture.name)
def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout
run('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=isolated-test-CA', '-keyout', str(base/'ca.key'), '-out', str(base/'ca.pem'))
run('openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=discord.com', '-keyout', str(base/'server.key'), '-out', str(base/'server.csr'))
(base/'extensions').write_text('basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:discord.com\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
run('openssl', 'x509', '-req', '-days', '1', '-in', str(base/'server.csr'), '-CA', str(base/'ca.pem'), '-CAkey', str(base/'ca.key'), '-CAcreateserial', '-extfile', str(base/'extensions'), '-out', str(base/'server.pem'))
seen = []
class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        seen.append(self.rfile.read(int(self.headers['Content-Length'])))
        self.send_response(204)
        self.end_headers()
    def log_message(self, *_):
        pass
server = http.server.HTTPServer(('127.0.0.1', 443), Handler)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(base/'server.pem', base/'server.key')
server.socket = ctx.wrap_socket(server.socket, server_side=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
cid = run('docker', 'run', '-d', '--network=host', '--add-host=discord.com:127.0.0.1', '-v', f'{base}/ca.pem:/test-ca.pem:ro', '-e', 'SSL_CERT_FILE=/test-ca.pem', '-e', 'SSL_CERT_DIR=/nonexistent', '-e', 'SMS_GATEWAY_SIGNING_KEY=isolated-test-key', '-e', 'DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/123/isolated-token', '-e', f'BIND_ADDR=127.0.0.1:{port}', image).strip()
try:
    url = f'http://127.0.0.1:{port}'
    for _ in range(100):
        try:
            urllib.request.urlopen(url+'/healthz', timeout=.2).close()
            break
        except OSError:
            time.sleep(.1)
    body = b'{"event":"sms:received","payload":{"sender":"current","phoneNumber":"legacy","message":"isolated TLS trust test"}}'
    ts = str(int(time.time()))
    signature = hmac.new(b'isolated-test-key', body+ts.encode(), hashlib.sha256).hexdigest()
    request = urllib.request.Request(url+'/webhook', data=body, headers={'X-Timestamp':ts,'X-Signature':signature})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    print(json.dumps({'image':image,'status':status,'local_mock_posts':len(seen),'expected_status':502,'external_provider_calls':0}), flush=True)
    assert status == 502, f'platform CA changed embedded-only trust: got {status}'
    assert not seen, 'a platform-only CA must never authorize forwarding'
finally:
    run('docker', 'rm', '-f', cid)
    server.shutdown()
