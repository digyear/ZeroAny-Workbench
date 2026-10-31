#!/usr/bin/env python3
"""MFA-style SSH fixture that mirrors the bastion the app kept failing against.

Run standalone: python3 mfa_ssh_server.py [port]

Auth policy (deliberately the same shape as the real host, per its logs):
  * keyboard-interactive BEFORE any password attempt -> USERAUTH_FAILURE,
    remaining methods advertise "password,publickey"
  * password method -> always rejected, but afterwards the server advertises
    "keyboard-interactive" (the "factor #1 accepted, give me the OTP" shape)
  * keyboard-interactive after that -> two prompt rounds: Password:, then
    Verification code: . Correct pair -> auth success.
"""
import socket
import sys
import threading

import paramiko

HOST, PORT = "127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 2222
# "simple" drops the MFA ladder and accepts any public key / the fixed
# password straight away. Used by the MCP ssh_exec end-to-end check, which
# drives the real app (no scripted prompt source there).
SIMPLE = len(sys.argv) > 2 and sys.argv[2] == "simple"
USER = "test"
PASSWORD = "pw1"
OTP = "123456"


class Server(paramiko.ServerInterface):
    def __init__(self):
        self.password_tried = False
        self.password_ok = False

    def check_auth_password(self, username, password):
        self.password_tried = True
        if SIMPLE and password == PASSWORD:
            return paramiko.AUTH_SUCCESSFUL
        return paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        self.password_tried = True
        if SIMPLE:
            return paramiko.AUTH_SUCCESSFUL
        return paramiko.AUTH_FAILED

    # keyboard-interactive -------------------------------------------------
    def check_auth_interactive(self, username, submethods):
        if not self.password_tried:
            # Host rejects a KBI-first attempt outright.
            return paramiko.AUTH_FAILED
        return paramiko.InteractiveQuery("", "", ("Password: ", False))

    def check_auth_interactive_response(self, responses):
        if len(responses) == 1 and not self.password_ok:
            if responses[0] == PASSWORD:
                self.password_ok = True
                return paramiko.InteractiveQuery(
                    "One more step", "", ("Verification code: ", False)
                )
            return paramiko.AUTH_FAILED
        if len(responses) == 1 and self.password_ok:
            return paramiko.AUTH_SUCCESSFUL if responses[0] == OTP else paramiko.AUTH_FAILED
        return paramiko.AUTH_FAILED

    def check_channel_request(self, kind, chanid):
        if kind == "session":
            return paramiko.OPEN_SUCCEEDED
        return paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def get_allowed_auths(self, username):
        if SIMPLE:
            return "publickey,password"
        if self.password_tried:
            return "keyboard-interactive"
        return "password,publickey"

    def check_channel_exec_request(self, channel, command):
        threading.Thread(target=self._reply, args=(channel, command), daemon=True).start()
        return True

    @staticmethod
    def _reply(channel, command):
        channel.sendall(b"EXEC OK: " + command + b"\r\n")
        channel.send_exit_status(0)
        channel.close()


def handle(conn):
    transport = paramiko.Transport(conn)
    host_key = paramiko.RSAKey.generate(2048)
    transport.add_server_key(host_key)
    server = Server()
    try:
        transport.start_server(server=server)
    except Exception as exc:  # noqa: BLE001
        print("handshake failed:", exc, flush=True)
        return
    chan = transport.accept(20)
    if chan is None:
        return
    while transport.is_active():
        try:
            chan.recv(1024)
        except Exception:  # noqa: BLE001
            break


def main():
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((HOST, PORT))
    srv.listen(5)
    mode = "simple" if SIMPLE else "mfa"
    print(
        f"fixture listening on {HOST}:{PORT} user={USER} pw={PASSWORD} otp={OTP} mode={mode}",
        flush=True,
    )
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=handle, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()