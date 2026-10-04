. ./env.sh
set -x
# Cold-ish start of a .NET Framework console program under Wine (the prefix's wineserver is already up).
wineserver -k; sleep 1
( time wine build/counter/Counter.exe 100000 > build/counter/out1.txt 2>&1 ) 2>&1 | tail -3 &
sleep 8; head -3 build/counter/out1.txt
wineserver -k; sleep 1
cat > build/hello.cs <<'C'
class P { static void Main() { System.Console.WriteLine("hello from CLR " + System.Environment.Version + " on " + System.Environment.OSVersion + " 64-bit=" + System.Environment.Is64BitProcess); } }
C
wine "$CSC" -nologo -out:build/hello.exe build/hello.cs
echo "--- hello.exe, wineserver cold"; wineserver -k; sleep 1; time wine build/hello.exe
echo "--- hello.exe, wineserver warm"; time wine build/hello.exe
echo "--- hello.exe, warm again"; time wine build/hello.exe
echo "--- real framework version"; wine reg query 'HKLM\Software\Microsoft\NET Framework Setup\NDP\v4\Full' /v Release; wine reg query 'HKLM\Software\Microsoft\NET Framework Setup\NDP\v4\Full' /v Version
