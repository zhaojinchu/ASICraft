#!/usr/bin/env bash
# Compiles the observer mod (mod/) against the Minecraft server, Fabric Loader and
# Mixin in .tools/fabric, with plain javac: Minecraft 26.3 ships unobfuscated, so
# no remapping and no Gradle. Writes build/mod/redfab-observer.jar, then runs the
# mod's own tests (mod/test, no Minecraft needed): the stream server must end
# its threads when their clients go.
set -euo pipefail
cd "$(dirname "$0")/.."
source ./versions.env
# The JDK's bin directory: next to the java that tools/setup.sh linked as .tools/java,
# or the macOS JDK layout from before the link existed.
if [ -L .tools/java ]; then
    JDK="$(cd .tools && dirname "$(readlink java)")"
    JDK=".tools/$JDK"
else
    JDK=.tools/jdk/Contents/Home/bin
fi
FABRIC=.tools/fabric
OUT=build/mod
SERVER_CLASSES="$FABRIC/versions/$MC_VERSION/server-$MC_VERSION.jar"
[ -f "$SERVER_CLASSES" ] || { echo "Fabric is not installed; run tools/setup.sh"; exit 1; }
[ -x "$JDK/javac" ] || { echo "no javac at $JDK; run tools/setup.sh"; exit 1; }

CP="$SERVER_CLASSES"
for jar in $(find "$FABRIC/libraries" -name '*.jar'); do
    CP="$CP:$jar"
done
rm -rf "$OUT" && mkdir -p "$OUT/classes"
"$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -cp "$CP" -d "$OUT/classes" $(find mod/src -name '*.java')
cp mod/resources/* "$OUT/classes/"
"$JDK/jar" --create --file "$OUT/redfab-observer.jar" -C "$OUT/classes" .
echo "built $OUT/redfab-observer.jar ($(find mod/src -name '*.java' | wc -l | tr -d ' ') sources)"

mkdir -p "$OUT/test-classes"
"$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -cp "$OUT/classes:$CP" -d "$OUT/test-classes" $(find mod/test -name '*.java')
for test in $(find mod/test -name '*Test.java' -exec basename {} .java \;); do
    "$JDK/java" -cp "$OUT/test-classes:$OUT/classes:$CP" "$test" || { echo "mod test $test failed"; exit 1; }
done

# --- the camera mod (mod-camera/, stage 14): a client mod. Its arithmetic and its
# backend client (mod-camera/src/dev/redfab/camera/*.java) know nothing of the game
# and are compiled and tested here always; the game-facing part (dev/redfab/camera/mc
# and the Mixins) needs the official client jar, which tools/setup.sh --client fetches
# to .tools/mc/client.jar for building here only. Without it the jar is not built, and
# this says so.
CAM=build/mod-camera
rm -rf "$CAM" && mkdir -p "$CAM/classes" "$CAM/test-classes"
"$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -d "$CAM/classes" $(find mod-camera/src/dev/redfab/camera -maxdepth 1 -name '*.java')
# The tests may use the observer mod's WebSocket server as a fake backend.
"$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -cp "$CAM/classes:$OUT/classes" -d "$CAM/test-classes" $(find mod-camera/test -name '*.java')
for test in $(find mod-camera/test -name '*Test.java' -exec basename {} .java \;); do
    "$JDK/java" -cp "$CAM/test-classes:$CAM/classes:$OUT/classes:$CP" "$test" || { echo "camera mod test $test failed"; exit 1; }
done
CLIENT_JAR=.tools/mc/client-$MC_VERSION.jar
if [ -f "$CLIENT_JAR" ] && [ -d mod-camera/src/dev/redfab/camera/mc ]; then
    "$JDK/javac" --release 25 -Xlint:all,-classfile -Werror -proc:none -cp "$CLIENT_JAR:$CP:$CAM/classes" -d "$CAM/classes" $(find mod-camera/src/dev/redfab/camera/mc -name '*.java')
    cp mod-camera/resources/* "$CAM/classes/"
    "$JDK/jar" --create --file "$OUT/redfab-camera.jar" -C "$CAM/classes" .
    echo "built $OUT/redfab-camera.jar ($(find mod-camera/src -name '*.java' | wc -l | tr -d ' ') sources)"
else
    echo "camera mod: arithmetic tested; the jar is not built here (no $CLIENT_JAR: tools/setup.sh --client fetches it)"
fi
