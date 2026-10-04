set -x
export DEBIAN_FRONTEND=noninteractive
dpkg --add-architecture i386
mkdir -pm755 /etc/apt/keyrings
curl -sSfL -o /etc/apt/keyrings/winehq-archive.key https://dl.winehq.org/wine-builds/winehq.key
curl -sSfL -o /etc/apt/sources.list.d/winehq-noble.sources https://dl.winehq.org/wine-builds/ubuntu/dists/noble/winehq-noble.sources
apt-get update
apt-get install -y --no-install-recommends winehq-stable cabextract xvfb xauth p7zip-full pdftotext poppler-utils mingw-w64 2>&1 | tail -5
wine --version
curl -sSfL -o /usr/local/bin/winetricks https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks && chmod +x /usr/local/bin/winetricks
winetricks --version
echo INSTALL-DONE
