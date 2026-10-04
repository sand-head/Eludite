. ./env.sh
set -x
rm -rf build/webapp && cp -r webhost/app build/webapp && mkdir -p build/webapp/bin && cp build/webhost/eludite-webhost.exe build/webapp/bin/
wine build/webhost/eludite-webhost.exe "Z:$(echo $S/build/webapp | tr / '\\')" http://127.0.0.1:8081/ > build/webhost.out 2> build/webhost.err &
for i in $(seq 1 60); do grep -q listening build/webhost.out && break; sleep 1; done
cat build/webhost.out
echo "--- first request (page compilation)"; time curl -sS -w "\nHTTP %{http_code} %{time_total}s\n" "http://127.0.0.1:8081/Default.aspx?name=Eludite" | tee build/page1.html | head -30
echo "--- second request"; curl -sS -o /dev/null -w "HTTP %{http_code} %{time_total}s\n" "http://127.0.0.1:8081/Default.aspx?name=again"
echo "--- third request"; curl -sS -o /dev/null -w "HTTP %{http_code} %{time_total}s\n" "http://127.0.0.1:8081/Default.aspx?name=again"
curl -sS -o /dev/null "http://127.0.0.1:8081/quit" || true
echo "--- stderr"; head -20 build/webhost.err
