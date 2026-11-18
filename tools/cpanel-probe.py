#!/usr/bin/env python3
"""Milestone 0 test: can Outpost reach the cPanel API on this host?

Tries the two sign-in options from the handoff doc and reports which work:

  1. The mailbox's own login against the webmail port (2096).
  2. A cPanel API token against the cPanel port (2083).

It only makes read-only calls (get_status, list_queued_messages,
get_spam_settings). It changes nothing on the server. Secrets are typed at a
hidden prompt, used for this run and never written anywhere. The output
contains no secrets and no message contents, so it is safe to paste back.

Run:  python3 tools/cpanel-probe.py        (Windows: py tools\\cpanel-probe.py)
Needs only the Python standard library.
"""

import base64
import getpass
import json
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20


def call(base, path, params, auth_header):
    """Returns (verdict, detail) for one read-only UAPI call."""
    url = f"{base}/execute/{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(url, headers={"Authorization": auth_header})
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT, context=ssl.create_default_context()) as reply:
            body = reply.read()
    except urllib.error.HTTPError as e:
        return "FAILED", f"HTTP {e.code} {e.reason}"
    except (urllib.error.URLError, OSError) as e:
        return "FAILED", f"could not connect: {getattr(e, 'reason', e)}"
    try:
        result = json.loads(body)["result"]
    except (ValueError, KeyError, TypeError):
        return "FAILED", "the reply was not a cPanel API result (a login page, probably)"
    if result.get("status") != 1:
        return "REFUSED", "; ".join(str(e) for e in (result.get("errors") or ["no reason given"]))
    return "OK", result.get("data")


def report(label, verdict, detail, describe):
    text = describe(detail) if verdict == "OK" else detail
    print(f"  [{verdict:^7}] {label}: {text}")
    return verdict == "OK"


def describe_status(data):
    return "BoxTrapper is ON for this mailbox" if data == 1 else "BoxTrapper is OFF for this mailbox"


def describe_queue(data):
    return f"{len(data or [])} message(s) held today"


def describe_spam(data):
    if not isinstance(data, dict):
        return "settings returned"
    # Keys only: enough to see which settings exist and which the host locks.
    return "settings returned: " + ", ".join(f"{k}={data[k]}" for k in sorted(data) if not isinstance(data[k], (dict, list)))


def run(base, email, auth_header):
    ok = report("BoxTrapper status", *call(base, "BoxTrapper/get_status", {"email": email}, auth_header), describe_status)
    ok &= report("BoxTrapper queue ", *call(base, "BoxTrapper/list_queued_messages", {"email": email}, auth_header), describe_queue)
    spam = report("Spam settings    ", *call(base, "Email/get_spam_settings", {}, auth_header), describe_spam)
    return ok, spam


def main():
    print(__doc__.split("\n\n")[0])
    print()
    host = input("Server hostname (what you use for webmail, e.g. mail.example.com): ").strip()
    email = input("Mailbox address: ").strip()
    if not host or not email:
        sys.exit("Hostname and mailbox address are both needed.")

    print("\nOption 1: mailbox login on the webmail port (2096)")
    password = getpass.getpass("  Mailbox password (hidden; Enter to skip): ")
    one = (False, False)
    if password:
        basic = base64.b64encode(f"{email}:{password}".encode()).decode()
        one = run(f"https://{host}:2096", email, f"Basic {basic}")
    else:
        print("  skipped")

    print("\nOption 2: cPanel API token on the cPanel port (2083)")
    print("  (cPanel > Security > Manage API Tokens. A token covers the whole hosting account.)")
    user = input("  cPanel username (Enter to skip): ").strip()
    two = (False, False)
    if user:
        token = getpass.getpass("  API token (hidden): ").strip()
        two = run(f"https://{host}:2083", email, f"cpanel {user}:{token}")
    else:
        print("  skipped")

    def words(result):
        return f"BoxTrapper {'yes' if result[0] else 'no'}, spam settings {'yes' if result[1] else 'no'}"

    print("\nSummary")
    print(f"  Mailbox login: {words(one)}")
    print(f"  API token:     {words(two)}")
    if not any(one + two):
        print("  Neither option worked. On this host the cPanel add-on is not viable as designed.")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit("\nCancelled.")
