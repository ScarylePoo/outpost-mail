#!/usr/bin/env python3
"""End-to-end self-test of the Outpost app (Linux).

Starts a throwaway Dovecot as the current user, puts some mail in it, runs the
real app against it in self-test mode, delivers one more message while the
window is open, and prints what the window showed.

    python3 tools/app-selftest.py [path/to/outpost]

Needs Dovecot 2.4 and a built app (`npm run build -- --no-bundle`). Uses only
temporary directories and a local port; touches no real mail or passwords.
"""

import imaplib
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

APP = sys.argv[1] if len(sys.argv) > 1 else "target/release/outpost"
DOVECOT = "/usr/sbin/dovecot"
PASSWORD = "testpass"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def message(n, subject, body):
    return (
        f"From: Sender {n} <sender{n}@example.com>\r\nTo: tester@example.com\r\n"
        f"Subject: {subject}\r\nDate: Mon, 2 Nov 2026 09:{n:02}:00 +0000\r\n"
        f"Message-ID: <{n}@selftest>\r\n\r\n{body}\r\n"
    ).encode()


def main():
    if not os.path.exists(DOVECOT):
        sys.exit("Dovecot is not installed.")
    if not os.path.exists(APP):
        sys.exit(f"No app at {APP}. Build it first, or pass its path.")
    work = tempfile.mkdtemp(prefix="outpost-selftest-")
    port = free_port()
    user = subprocess.check_output(["id", "-un"], text=True).strip()
    group = subprocess.check_output(["id", "-gn"], text=True).strip()
    version = subprocess.check_output([DOVECOT, "--version"], text=True).split()[0]
    for sub in ("run", "state", "home", "data"):
        os.makedirs(os.path.join(work, sub))
    conf = os.path.join(work, "dovecot.conf")
    with open(conf, "w") as f:
        f.write(f"""dovecot_config_version = {version}
dovecot_storage_version = {version}
base_dir = {work}/run
state_dir = {work}/state
instance_name = outpost-selftest-{port}
log_path = {work}/dovecot.log
protocols = imap
listen = 127.0.0.1
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

        report = os.path.join(work, "report.txt")
        env = dict(
            os.environ,
            OUTPOST_DATA=os.path.join(work, "data"),
            OUTPOST_SHELL_TEST=report,
            OUTPOST_PROBE_ACCOUNT=json.dumps({
                "host": "127.0.0.1", "port": port, "encrypted": False,
                "username": "tester", "password": PASSWORD, "remember": False,
            }),
            WEBKIT_DISABLE_DMABUF_RENDERER="1",
        )
        app = subprocess.Popen([APP], env=env, **quiet)
        deadline = time.time() + 60
        delivered = False
        while app.poll() is None and time.time() < deadline:
            if not delivered and os.path.exists(report) and open(report).read() == "READY":
                other.append("INBOX", "", None, message(30, "Arrived while open", "A late zebra."))
                delivered = True
            time.sleep(0.1)
        if app.poll() is None:
            app.kill()
            print("FAILED: the app did not finish within a minute.")
        print(open(report).read() if os.path.exists(report) else "FAILED: no report was written.")
        saved = os.path.join(work, "data", "accounts", "tester", "account.json")
        if os.path.exists(saved):
            text = open(saved).read()
            print("account.json holds the password!" if PASSWORD in text else "account.json: saved, no password in it")
    finally:
        subprocess.run([DOVECOT, "-c", conf, "stop"], **quiet)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
