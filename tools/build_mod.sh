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
