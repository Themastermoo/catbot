"""A tiny IMAP server for tests: two messages, plain on 127.0.0.1; and a tiny SMTP server that
appends what it's sent to the file named by FAKE_SMTP_LOG. Prints both ports, then serves.

The password is "right". It answers LOGIN, EXAMINE/SELECT, UID SEARCH (ALL, TEXT, FROM, UNSEEN,
UID n:*), UID FETCH (headers or whole messages), LIST and LOGOUT, which is what Catbot asks.
New mail: each .eml file in the folder FAKE_NEW_MAIL names (if set) arrives in INBOX, in name
order, the next time a folder is opened.
"""
import os, re, socket, sys, threading

MESSAGES = {
    1: (True, b"From: Bob <bob@x.com>\r\nTo: Moo <moo@y.com>\r\nSubject: Seeds\r\nDate: Mon, 16 Nov 2026 09:00:00 -0600\r\nMessage-ID: <seeds@x.com>\r\n\r\nThe seeds came today.\r\n"),
    2: (False, b"From: Ann <ann@z.com>\r\nTo: Moo <moo@y.com>\r\nSubject: Invoice 42\r\nDate: Mon, 16 Nov 2026 10:30:00 -0600\r\nMessage-ID: <invoice@z.com>\r\n\r\nPlease pay invoice 42.\r\n"),
}

def take_new_mail():
    folder = os.environ.get("FAKE_NEW_MAIL")
    if not folder or not os.path.isdir(folder): return
    for name in sorted(os.listdir(folder)):
        if name.endswith(".eml"):
            path = os.path.join(folder, name)
            with open(path, "rb") as f:
                MESSAGES[max(MESSAGES) + 1] = (False, f.read())
            os.remove(path)

def matches(uid, criteria):
    seen, raw = MESSAGES[uid]
    text = raw.decode().lower()
    head, _, _ = text.partition("\r\n\r\n")
    for key, value in re.findall(r'(TEXT|FROM|SUBJECT) "([^"]*)"', criteria):
        value = value.lower()
        if key == "TEXT" and value not in text: return False
        if key == "FROM" and not re.search(r"^from:.*" + re.escape(value), head, re.M): return False
        if key == "SUBJECT" and not re.search(r"^subject:.*" + re.escape(value), head, re.M): return False
    if "UNSEEN" in criteria and seen: return False
    since = re.search(r"UID (\d+):\*", criteria)
    if since and uid < int(since.group(1)):
        # As real servers do, "n:*" still includes the newest message when nothing is that new.
        return uid == max(MESSAGES)
    return True

def serve(conn):
    f = conn.makefile("rb")
    send = lambda b: conn.sendall(b if isinstance(b, bytes) else b.encode())
    send("* OK fake IMAP ready\r\n")
    while True:
        line = f.readline()
        if not line: break
        line = line.decode().rstrip("\r\n")
        tag, _, rest = line.partition(" ")
        cmd = rest.upper()
        if cmd.startswith("LOGIN"):
            if rest.split()[-1].strip('"') == "right":
                send(f"{tag} OK LOGIN completed\r\n")
            else:
                send(f"{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
        elif cmd.startswith("CAPABILITY"):
            send(f"* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n")
        elif cmd.startswith("EXAMINE") or cmd.startswith("SELECT"):
            if "INBOX" not in rest.upper():
                send(f"{tag} NO no such folder\r\n"); continue
            take_new_mail()
            send(f"* {len(MESSAGES)} EXISTS\r\n* 0 RECENT\r\n* FLAGS (\\Seen)\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {max(MESSAGES) + 1}] next\r\n{tag} OK [READ-ONLY] done\r\n")
        elif cmd.startswith("UID SEARCH"):
            criteria = rest[len("UID SEARCH "):]
            found = [str(u) for u in MESSAGES if matches(u, criteria)]
            send(f"* SEARCH {' '.join(found)}\r\n{tag} OK SEARCH completed\r\n")
        elif cmd.startswith("UID FETCH"):
            parts = rest.split(" ", 3)
            uids = [int(u) for u in parts[2].split(",") if int(u) in MESSAGES]
            whole = "BODY.PEEK[]" in rest
            for uid in uids:
                seen, raw = MESSAGES[uid]
                data = raw if whole else raw.split(b"\r\n\r\n")[0] + b"\r\n\r\n"
                section = "BODY[]" if whole else "BODY[HEADER]"
                flags = "\\Seen" if seen else ""
                send(f"* {uid} FETCH (UID {uid} FLAGS ({flags}) {section} {{{len(data)}}}\r\n".encode() + data + b")\r\n")
            send(f"{tag} OK FETCH completed\r\n")
        elif cmd.startswith("LIST"):
            send(f'* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren) "/" "Archive"\r\n{tag} OK LIST completed\r\n')
        elif cmd.startswith("LOGOUT"):
            send(f"* BYE bye\r\n{tag} OK LOGOUT completed\r\n"); break
        elif cmd.startswith("NOOP"):
            send(f"{tag} OK NOOP\r\n")
        else:
            send(f"{tag} BAD unknown\r\n")
    conn.close()

def serve_smtp(conn):
    import os
    f = conn.makefile("rb")
    send = lambda s: conn.sendall(s.encode())
    send("220 fake SMTP ready\r\n")
    envelope = []
    while True:
        line = f.readline()
        if not line: break
        cmd = line.decode().rstrip("\r\n")
        upper = cmd.upper()
        if upper.startswith("EHLO") or upper.startswith("HELO"):
            send("250-fake\r\n250 AUTH PLAIN LOGIN\r\n")
        elif upper.startswith("AUTH PLAIN"):
            import base64
            parts = base64.b64decode(cmd.split()[-1]).split(b"\0")
            send("235 ok\r\n" if parts[-1] == b"right" else "535 5.7.8 bad credentials\r\n")
        elif upper.startswith("MAIL FROM") or upper.startswith("RCPT TO"):
            envelope.append(cmd)
            send("250 ok\r\n")
        elif upper == "DATA":
            send("354 go\r\n")
            data = b""
            while not data.endswith(b"\r\n.\r\n"):
                data += f.readline()
            if os.environ.get("FAKE_SMTP_LOG"):
                with open(os.environ["FAKE_SMTP_LOG"], "ab") as log:
                    log.write("\n".join(envelope).encode() + b"\n" + data + b"\n=====\n")
            envelope = []
            send("250 queued\r\n")
        elif upper == "QUIT":
            send("221 bye\r\n"); break
        else:
            send("250 ok\r\n")
    conn.close()

def listen(handler):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    def loop():
        while True:
            conn, _ = listener.accept()
            threading.Thread(target=handler, args=(conn,), daemon=True).start()
    threading.Thread(target=loop, daemon=True).start()
    return listener.getsockname()[1]

imap_port, smtp_port = listen(serve), listen(serve_smtp)
print(imap_port, smtp_port, flush=True)
threading.Event().wait()
