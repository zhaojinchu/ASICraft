#!/usr/bin/env bash
# Compiles the observer mod (mod/) against the Minecraft server, Fabric Loader and
# Mixin in .tools/fabric, with plain javac: Minecraft 26.3 ships unobfuscated, so
# no remapping and no Gradle. Writes build/mod/redfab-observer.jar.
set -euo pipefail
cd "$(dirname "$0")/.."
source ./versions.env
JDK=.tools/jdk/Contents/Home/bin
FABRIC=.tools/fabric
OUT=build/mod
SERVER_CLASSES="$FABRIC/versions/$MC_VERSION/server-$MC_VERSION.jar"
[ -f "$SERVER_CLASSES" ] || { echo "Fabric is not installed; run tools/setup.sh"; exit 1; }

CP="$SERVER_CLASSES"
for jar in $(find "$FABRIC/libraries" -name '*.jar'); do
    CP="$CP:$jar"
done
rm -rf "$OUT" && mkdir -p "$OUT/classes"
"$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -cp "$CP" -d "$OUT/classes" $(find mod/src -name '*.java')
cp mod/resources/* "$OUT/classes/"
"$JDK/jar" --create --file "$OUT/redfab-observer.jar" -C "$OUT/classes" .
echo "built $OUT/redfab-observer.jar ($(find mod/src -name '*.java' | wc -l | tr -d ' ') sources)"
