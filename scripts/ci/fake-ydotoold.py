"""Bind a do-nothing socket where ydotool's client looks for ydotoold.

Both ydotool versions connect to the daemon before parsing arguments and
exit without printing `key --help` if that fails. CI containers have no
/dev/uinput to run a real ydotoold, so the client just needs something to
connect to. 0.1.x uses a stream socket, 1.x a datagram socket.

Usage: python3 fake-ydotoold.py <stream|dgram> <path>   (runs until killed)
"""
import os
import signal
import socket
import sys

kind, path = sys.argv[1], sys.argv[2]
sock_type = socket.SOCK_STREAM if kind == "stream" else socket.SOCK_DGRAM
if os.path.exists(path):
    os.unlink(path)
s = socket.socket(socket.AF_UNIX, sock_type)
s.bind(path)
os.chmod(path, 0o666)
if sock_type == socket.SOCK_STREAM:
    s.listen(8)
print("ready", flush=True)
signal.pause()
