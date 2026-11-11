"""Run a command in its own network namespace (started by `unshare -rn`) with a stand-in for Heimdall's API on 127.0.0.1:8930,
so the check-up (which asks that fixed address) never reaches the real Heimdall.

  unshare -rn python3 heimdall_ns.py AUDIT.json QUARANTINE.json LOG -- COMMAND...

A missing answer file is a 404. Each request's method and path is written to LOG. The exit status is the command's."""
import http.server
import os
import shutil
import subprocess
import sys
import threading

audit, quarantine, log = sys.argv[1:4]
cmd = sys.argv[sys.argv.index("--") + 1:]
subprocess.run([shutil.which("ip", path="/usr/bin:/bin:/usr/sbin:/sbin") or "ip", "link", "set", "lo", "up"], check=True)


class Heimdall(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write("GET %s\n" % self.path)
        path = {"/v1/audit": audit, "/v1/quarantine": quarantine}.get(self.path)
        try:
            with open(path, "rb") as f:
                data = f.read()
        except (OSError, TypeError):
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 8930), Heimdall)
threading.Thread(target=server.serve_forever, daemon=True).start()
r = subprocess.run(cmd)
server.shutdown()
sys.exit(r.returncode if r.returncode >= 0 else 128 - r.returncode)
