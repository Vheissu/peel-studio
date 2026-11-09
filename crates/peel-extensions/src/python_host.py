"""JSON bridge for trusted Python scripts. Not a Python sandbox."""
import contextlib
import json
import sys
import types
import uuid

# One JSON value per line. The first stdin line is the payload; each later
# line answers one host-call frame this process wrote to stdout.
channel_in = sys.stdin.buffer
channel_out = sys.stdout.buffer
payload = json.loads(channel_in.readline())
logs = []
log_bytes = 0


def log(text):
    global log_bytes
    if log_bytes >= 16384 or len(logs) >= 256:
        return
    text = str(text).encode("utf-8")[:16384 - log_bytes].decode("utf-8", errors="ignore")
    logs.append(text)
    log_bytes += len(text.encode("utf-8")) + 1


class Output:
    def __init__(self):
        self.pending = ""

    def write(self, text):
        self.pending += text[:16384]
        while "\n" in self.pending:
            line, self.pending = self.pending.split("\n", 1)
            log(line)
        self.pending = self.pending[:16384]
        return len(text)

    def flush(self):
        if self.pending:
            log(self.pending)
            self.pending = ""


class HostError(RuntimeError):
    """A live host call failed. Earlier live calls are not rolled back."""


def send(value):
    channel_out.write(json.dumps(value, separators=(",", ":"), allow_nan=False).encode("ascii") + b"\n")
    channel_out.flush()


def call(name, args=None):
    if args is None:
        args = {}
    if not isinstance(name, str):
        raise TypeError("peel.call needs an action name")
    if not isinstance(args, dict):
        raise TypeError("peel.call arguments must be a dictionary")
    send({"peel_host_call": {"name": name, "args": args}})
    line = channel_in.readline()
    if not line:
        raise HostError("The host closed the script channel")
    reply = json.loads(line)
    if "error" in reply:
        raise HostError(reply["error"])
    return reply["ok"]


peel = types.SimpleNamespace(
    apiVersion=1,
    project=payload["project"],
    context=payload["context"],
    newId=lambda: uuid.uuid4().hex,
    log=log,
    call=call,
    HostError=HostError,
)
output = Output()
with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
    exec(compile(payload["source"], "script.py", "exec"), {"peel": peel, "__name__": "__main__"})
output.flush()
send({"project": peel.project, "logs": logs})
