"""A tiny IMAP server for tests: two messages, plain on 127.0.0.1. Prints its port, then serves.

The password is "right". It answers LOGIN, EXAMINE/SELECT, UID SEARCH (ALL, TEXT, FROM, UNSEEN),
UID FETCH (headers or whole messages), LIST and LOGOUT, which is what Catbot asks.
"""
import re, socket, sys, threading

MESSAGES = {
    1: (True, b"From: Bob <bob@x.com>\r\nTo: Moo <moo@y.com>\r\nSubject: Seeds\r\nDate: Mon, 16 Nov 2026 09:00:00 -0600\r\nMessage-ID: <seeds@x.com>\r\n\r\nThe seeds came today.\r\n"),
    2: (False, b"From: Ann <ann@z.com>\r\nTo: Moo <moo@y.com>\r\nSubject: Invoice 42\r\nDate: Mon, 16 Nov 2026 10:30:00 -0600\r\nMessage-ID: <invoice@z.com>\r\n\r\nPlease pay invoice 42.\r\n"),
}

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
            send(f"* {len(MESSAGES)} EXISTS\r\n* 0 RECENT\r\n* FLAGS (\\Seen)\r\n* OK [UIDVALIDITY 1] ok\r\n{tag} OK [READ-ONLY] done\r\n")
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

listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen()
print(listener.getsockname()[1], flush=True)
while True:
    conn, _ = listener.accept()
    threading.Thread(target=serve, args=(conn,), daemon=True).start()
