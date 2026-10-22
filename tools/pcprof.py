"""Instruction-level sampling profiler for ruruby, built on lldb's Python API
(for macOS without xctrace/dtrace; `sample` only reports whole functions).

It runs the program under lldb, interrupts it every few milliseconds and
records the program counter: per function, per function+offset (match it
against `disassemble`) and per source line. Each sample costs ~0.1 s of
debugger overhead, so 1000 samples take a couple of minutes; the program
only advances ~3 ms between samples, so give it a long-running workload.

Build with line tables (codegen is unchanged) and a dSYM, then run:

    CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
    CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed \
        cargo build --release --target-dir target/prof
    SAMPLES=1000 PYTHONPATH=$(lldb -P) /usr/bin/python3 tools/pcprof.py \
        prof.json target/prof/release/ruruby bench.rb
    python3 tools/pcprof.py --report prof.json

The disassembly of the dispatch loop, to read the hot offsets against:

    lldb --batch -o "disassemble --force -m -n '<ruruby::vm::Vm>::dispatch'" \
        target/prof/release/ruruby > dispatch.s
"""
import sys

if len(sys.argv) == 3 and sys.argv[1] == '--report':
    import json
    d = json.load(open(sys.argv[2]))
    n = d['samples']
    print('samples', n)
    for title, key in (('by source line', 'lines'), ('by function', 'funcs'), ('by address', 'pcs')):
        print(f'\n{title}:')
        for loc, c in d[key][:25]:
            print(f'{100 * c / n:5.1f}%  {loc}')
    sys.exit()

import json, sys, time, collections
import lldb
out, exe, args = sys.argv[1], sys.argv[2], sys.argv[3:]
dbg = lldb.SBDebugger.Create()
dbg.SetAsync(False)
target = dbg.CreateTarget(exe)
info = lldb.SBLaunchInfo(args)
info.SetLaunchFlags(lldb.eLaunchFlagStopAtEntry)
err = lldb.SBError()
proc = target.Launch(info, err)
assert err.Success(), err
import threading
lines, funcs, pcs = collections.Counter(), collections.Counter(), collections.Counter()
n = 0
limit = int(__import__('os').environ.get('SAMPLES', '1500'))
def interrupt_soon():
    time.sleep(0.003)
    proc.SendAsyncInterrupt()
while n < limit:
    threading.Thread(target=interrupt_soon).start()
    proc.Continue()  # synchronous: returns when the interrupt stops it
    if proc.GetState() != lldb.eStateStopped:
        break
    th = None
    for i in range(proc.GetNumThreads()):
        t = proc.GetThreadAtIndex(i)
        name = t.GetFrameAtIndex(0).GetFunctionName() or ''
        if not any(w in name for w in ('ulock', 'wait', 'join', 'kevent', 'semaphore')):
            th = t
            break
    if th is None:
        continue
    fr = th.GetFrameAtIndex(0)
    addr = fr.GetPCAddress()
    pc = addr.GetLoadAddress(target)
    fn = fr.GetFunctionName() or '?'
    sym = addr.GetSymbol()
    off = pc - sym.GetStartAddress().GetLoadAddress(target) if sym.IsValid() else 0
    le = addr.GetLineEntry()
    loc = f"{le.GetFileSpec().GetFilename()}:{le.GetLine()}" if le.IsValid() else '?'
    lines[loc] += 1
    funcs[fn] += 1
    pcs[f"{fn}+{off:#x}"] += 1
    n += 1
proc.Kill()
json.dump({'samples': n, 'lines': lines.most_common(), 'funcs': funcs.most_common(), 'pcs': pcs.most_common()}, open(out, 'w'))
print('samples', n)
