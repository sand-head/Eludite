# Shared by the spike scripts. S is the spike directory holding prefix/ (the Wine prefix), tools/ (the Roslyn
# toolset and reference-assembly nupkgs, unzipped) and build/ (outputs). Run the scripts from that directory.
export S=${S:-$PWD}
export WINEARCH=win64 WINEPREFIX=$S/prefix WINEDEBUG=-all
export CSC="$S/tools/toolset/tasks/net472/csc.exe"
export FX="$WINEPREFIX/drive_c/windows/Microsoft.NET/Framework64/v4.0.30319"
