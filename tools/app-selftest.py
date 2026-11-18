#!/usr/bin/env python3
"""End-to-end self-test of the Outpost app (Linux).

Starts a throwaway Dovecot as the current user, puts some mail in it, runs the
real app against it in self-test mode, delivers one more message while the
window is open, and prints what the window showed.

    python3 tools/app-selftest.py [path/to/outpost]

Needs Dovecot 2.4 and a built app (`npm run build -- --no-bundle`). Uses only
temporary directories and a local port; touches no real mail or passwords.

Under WSL it can also test the Windows build: pass the path to outpost.exe.
The Windows app then reaches the Dovecot inside WSL through localhost.
"""

import base64
import imaplib
import json
import os
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

APP = sys.argv[1] if len(sys.argv) > 1 else "target/release/outpost"
DOVECOT = "/usr/sbin/dovecot"
PASSWORD = "testpass"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


SENT = []


class FakeSmtp(socketserver.StreamRequestHandler):
    """Accepts mail and keeps it. Delivers nothing anywhere."""

    def handle(self):
        say = lambda text: self.wfile.write((text + "\r\n").encode())
        say("220 selftest ready")
        sender, recipients = "", []
        while True:
            line = self.rfile.readline().decode(errors="replace").strip()
            upper = line.upper()
            if not line or upper == "QUIT":
                say("221 bye")
                return
            if upper.startswith("EHLO"):
                self.wfile.write(b"250-selftest\r\n250 AUTH PLAIN\r\n")
            elif upper.startswith("AUTH PLAIN"):
                ok = base64.b64decode(line[11:]).split(b"\0")[-1].decode() == PASSWORD
                say("235 welcome" if ok else "535 no")
            elif upper.startswith("MAIL FROM:"):
                sender, recipients = line[10:].strip("<>"), []
                say("250 ok")
            elif upper.startswith("RCPT TO:"):
                recipients.append(line[8:].strip("<>"))
                say("250 ok")
            elif upper == "DATA":
                say("354 go on")
                data = b""
                while True:
                    part = self.rfile.readline()
                    if part in (b".\r\n", b""):
                        break
                    data += part
                SENT.append((sender, recipients, data.decode(errors="replace")))
                say("250 accepted")
            else:
                say("502 not implemented")


class Threaded(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


def message(n, subject, body):
    return (
        # Sender checks as the receiving server would record them. Message 2
        # is a forgery: it fails.
        f"Authentication-Results: mx.selftest.example; "
        f"dmarc={'fail' if n == 2 else 'pass'} header.from=example.com\r\n"
        f"From: Sender {n} <sender{n}@example.com>\r\nTo: tester@example.com\r\n"
        f"Subject: {subject}\r\nDate: Mon, 2 Nov 2026 09:{n:02}:00 +0000\r\n"
        f"Message-ID: <{n}@selftest>\r\n\r\n{body}\r\n"
    ).encode()


def main():
    if not os.path.exists(DOVECOT):
        sys.exit("Dovecot is not installed.")
    if not os.path.exists(APP):
        sys.exit(f"No app at {APP}. Build it first, or pass its path.")
    windows = APP.lower().endswith(".exe")
    work = tempfile.mkdtemp(prefix="outpost-selftest-")
    # The Windows app needs its files somewhere Windows can see.
    shared = tempfile.mkdtemp(prefix="outpost-selftest-", dir="/mnt/c/Users/Public") if windows else work

    def native(path):
        return subprocess.check_output(["wslpath", "-w", path], text=True).strip() if windows else path

    port = free_port()
    user = subprocess.check_output(["id", "-un"], text=True).strip()
    group = subprocess.check_output(["id", "-gn"], text=True).strip()
    version = subprocess.check_output([DOVECOT, "--version"], text=True).split()[0]
    for sub in ("run", "state", "home"):
        os.makedirs(os.path.join(work, sub))
    os.makedirs(os.path.join(shared, "data"))
    conf = os.path.join(work, "dovecot.conf")
    with open(conf, "w") as f:
        f.write(f"""dovecot_config_version = {version}
dovecot_storage_version = {version}
base_dir = {work}/run
state_dir = {work}/state
instance_name = outpost-selftest-{port}
log_path = {work}/dovecot.log
protocols = imap
listen = {'*' if windows else '127.0.0.1'}
default_internal_user = {user}
default_internal_group = {group}
default_login_user = {user}
first_valid_uid = 1
ssl = no
auth_allow_cleartext = yes
mail_driver = maildir
mail_home = {work}/home/%{{user}}
mail_path = ~/Maildir
mail_uid = {user}
mail_gid = {group}
passdb static {{
  password = {PASSWORD}
}}
userdb static {{
}}
namespace inbox {{
  inbox = yes
  separator = /
  mailbox Sent {{
    special_use = \\Sent
    auto = subscribe
  }}
  mailbox Trash {{
    special_use = \\Trash
    auto = subscribe
  }}
}}
service imap-login {{
  chroot =
  inet_listener imap {{
    port = {port}
  }}
  inet_listener imaps {{
    port = 0
  }}
}}
service anvil {{
  chroot =
}}
service imap {{
  chroot =
}}
service stats {{
  chroot =
}}
""")
    quiet = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run([DOVECOT, "-c", conf], check=True, **quiet)
    try:
        for _ in range(100):
            try:
                other = imaplib.IMAP4("127.0.0.1", port)
                break
            except OSError:
                time.sleep(0.1)
        other.login("tester", PASSWORD)
        other.create("Projects")
        for n in range(1, 6):
            other.append("INBOX", "(\\Seen)" if n % 2 else "", None, message(n, f"Selftest message {n}", f"Body {n} with a zebra in it."))
        other.append("Projects", "", None, message(9, "Project note", "No striped animals here."))

        # From Windows, the server inside WSL is reached at WSL's own address.
        host = subprocess.check_output(["hostname", "-I"], text=True).split()[0] if windows else "127.0.0.1"
        smtp = Threaded(("0.0.0.0" if windows else "127.0.0.1", 0), FakeSmtp)
        threading.Thread(target=smtp.serve_forever, daemon=True).start()
        report = os.path.join(shared, "report.txt")
        env = dict(
            os.environ,
            OUTPOST_DATA=native(os.path.join(shared, "data")),
            OUTPOST_SHELL_TEST=native(report),
            # Lets these variables cross from WSL into a Windows program.
            WSLENV="OUTPOST_DATA:OUTPOST_SHELL_TEST:OUTPOST_PROBE_ACCOUNT",
            OUTPOST_PROBE_ACCOUNT=json.dumps({
                "host": host, "port": port, "encrypted": False,
                "username": "tester", "password": PASSWORD, "remember": False,
                "sending": {
                    "name": "Self Test", "address": "tester@example.com",
                    "host": host, "port": smtp.server_address[1], "security": "plaintext",
                },
            }),
            WEBKIT_DISABLE_DMABUF_RENDERER="1",
        )
        app = subprocess.Popen([APP], env=env, **quiet)
        deadline = time.time() + 90
        delivered = False
        while app.poll() is None and time.time() < deadline:
            if not delivered and os.path.exists(report) and open(report).read() == "READY":
                other.append("INBOX", "", None, message(30, "Arrived while open", "A late zebra."))
                delivered = True
            time.sleep(0.1)
        if app.poll() is None:
            app.kill()
            print("FAILED: the app did not finish in time.")
        print(open(report).read() if os.path.exists(report) else "FAILED: no report was written.")
        # What the app did must have reached the server.
        def on_server(folder):
            other.select(f'"{folder}"', readonly=True)
            _, data = other.uid("FETCH", "1:*", "(FLAGS BODY.PEEK[HEADER.FIELDS (SUBJECT)])")
            found = []
            for part in data or []:
                if isinstance(part, tuple):
                    flags = part[0].decode().split("FLAGS (")[1].split(")")[0]
                    flags = " ".join(sorted(f for f in flags.split() if not f.startswith("$") and f != "\\Recent"))
                    found.append((part[1].decode().replace("Subject: ", "").strip(), flags))
            return sorted(found)
        print("server INBOX:", on_server("INBOX"))
        print("server Trash:", on_server("Trash"))
        print("server Sent: ", on_server("Sent"))
        for sender, recipients, data in SENT:
            subject = next((l for l in data.splitlines() if l.startswith("Subject:")), "")
            said = "Written in the window." in data
            print(f"handed to the outgoing server: from {sender} to {recipients}; {subject}; body arrived: {said}")
        if not SENT:
            print("handed to the outgoing server: NOTHING")
        saved = os.path.join(shared, "data", "accounts", "tester", "account.json")
        if os.path.exists(saved):
            text = open(saved).read()
            print("account.json holds the password!" if PASSWORD in text else "account.json: saved, no password in it")
    finally:
        subprocess.run([DOVECOT, "-c", conf, "stop"], **quiet)
        shutil.rmtree(work, ignore_errors=True)
        shutil.rmtree(shared, ignore_errors=True)


if __name__ == "__main__":
    main()
