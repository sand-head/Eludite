export WINEARCH=win64 WINEPREFIX=$PWD/prefix WINEDEBUG=-all W_CACHE=$PWD/cache
export WINEDLLOVERRIDES="mscoree=d"   # no wine-mono in this prefix
date
time xvfb-run -a winetricks -q --force dotnet48 2>&1 | tail -40
date
ls prefix/drive_c/windows/Microsoft.NET/Framework64/ 2>/dev/null
echo DOTNET48-DONE
