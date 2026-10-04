. ./env.sh
set -x
mkdir -p build/counter build/webhost
cp $(git rev-parse --show-toplevel)/debuggers/netfx/fixtures/Counter/Program.cs build/counter/Program.cs
# Roslyn csc (net472) on the real framework under Wine: a portable PDB, x64, no optimization, as Counter.csproj does.
time wine "$CSC" -nologo -debug:portable -platform:x64 -optimize- -langversion:7.3 -out:build/counter/Counter.exe build/counter/Program.cs
ls -la build/counter
time wine "$CSC" -nologo -debug:portable -platform:x64 -r:System.Web.dll -out:build/webhost/eludite-webhost.exe webhost/Host.cs
ls -la build/webhost
