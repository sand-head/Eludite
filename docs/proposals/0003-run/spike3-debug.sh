. ./env.sh
set -x
ADAPTER=$(git rev-parse --show-toplevel)/target/x86_64-pc-windows-gnu/release/eludite-dbg-netfx.exe
LINE=$(grep -n "// BREAKPOINT" build/counter/Program.cs | cut -d: -f1)
wine build/counter/Counter.exe 50 > build/counter/ticks.txt 2>&1 &
sleep 3
PID=$(grep -m1 ready build/counter/ticks.txt | awk '{print $2}')
echo "debuggee wine pid $PID"
wine "$ADAPTER" --listen 127.0.0.1:47110 2> build/adapter.err &
sleep 3
cat build/adapter.err
timeout 120 python3 dap/client.py 127.0.0.1 47110 "$PID" "Z:$(echo $S/build/counter/Program.cs | tr / '\\')" "$LINE"
echo "exit $?"
sleep 1; tail -3 build/counter/ticks.txt
echo "--- adapter stderr"; cat build/adapter.err
wineserver -k
