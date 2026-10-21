"""Optional Unix PTY regression: cargo build && python3 tests/pty_smoke.py.
Uses a mock xctrace; no profiling privileges or Xcode required.
Requires permission to change the PTY foreground process group.
"""
import os, pty, select, signal, tempfile, time
from pathlib import Path
root=Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='mperf-terminal-') as d:
    d=Path(d)
    mock=(root/'tests/fixtures/xcrun.sh').read_text()
    mock=mock.replace("trap 'exit 0' INT", "printf 'prompt> '; read reply\n    stopped=0\n    trap 'if [ \"$stopped\" = 1 ]; then exit 42; fi; stopped=1' INT")
    mock=mock.replace('while :; do sleep 0.05; done', 'while [ "$stopped" = 0 ]; do sleep 0.05; done\n    sleep 0.2\n    exit 0')
    (d/'xcrun').write_text(mock)
    (d/'xcrun').chmod(0o755)
    env=dict(os.environ, PATH=str(d)+':'+os.environ['PATH'], MPERF_WAIT='1', MPERF_READY=str(d/'ready'), MPERF_FIXTURE=str(root/'tests/fixtures/before.xml'))
    pid,fd=pty.fork()
    if pid==0:
        os.chdir(d)
        os.execve(root/'target/debug/mperf',['mperf','record','-g','-p','123'],env)
    deadline=time.monotonic()+8
    transcript=b''
    replied=stopped=False
    try:
        while time.monotonic()<deadline:
            r,_,_=select.select([fd],[],[],0.05)
            if r:
                try: transcript+=os.read(fd,65536)
                except OSError: pass
            if b'prompt> ' in transcript and not replied:
                os.write(fd,b'yes\n'); replied=True
            if (d/'ready').exists() and not stopped:
                os.write(fd,b'\x03'); stopped=True
            done,status=os.waitpid(pid,os.WNOHANG)
            if done:
                assert os.waitstatus_to_exitcode(status)==0, transcript.decode(errors='replace')
                assert (d/'mperf.trace/samples.xml').exists(), transcript
                print('PASS: foreground prompt accepted input; terminal Ctrl-C saved the trace without a duplicate interrupt.')
                break
        else: raise AssertionError('timeout: '+transcript.decode(errors='replace'))
    finally:
        try: os.killpg(pid,signal.SIGKILL)
        except ProcessLookupError: pass
        os.close(fd)
