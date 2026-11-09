"""JSON bridge for trusted Python scripts. Not a Python sandbox."""
import contextlib
import json
import sys
import types
import uuid

payload = json.load(sys.stdin.buffer)
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


peel = types.SimpleNamespace(
    apiVersion=1,
    project=payload["project"],
    context=payload["context"],
    newId=lambda: uuid.uuid4().hex,
    log=log,
)
output = Output()
with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
    exec(compile(payload["source"], "script.py", "exec"), {"peel": peel, "__name__": "__main__"})
output.flush()
json.dump({"project": peel.project, "logs": logs}, sys.stdout, allow_nan=False)
