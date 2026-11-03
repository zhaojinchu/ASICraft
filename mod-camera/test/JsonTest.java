// The mod's JSON, enough for the backend's messages: objects, arrays, strings
// with escapes, numbers, booleans and null, nested; a refusal on anything
// else; and writing what the mod sends.
import java.util.List;
import java.util.Map;

import dev.redfab.camera.Json;

public final class JsonTest {
    private static int failures = 0;

    private static void check(boolean ok, String what) {
        if (!ok) {
            failures++;
            System.out.println("FAIL: " + what);
        }
    }

    @SuppressWarnings("unchecked")
    public static void main(String[] args) {
        Object parsed = Json.parse("{\"frame\": {\"plot\": 3, \"origin\": [1024, 64, 0], \"size\": [94, 4, 68]}, \"ok\": true, \"why\": null}");
        check(parsed instanceof Map, "an object parses to a map");
        Map<String, Object> top = (Map<String, Object>) parsed;
        Map<String, Object> frame = (Map<String, Object>) top.get("frame");
        check(((Number) frame.get("plot")).intValue() == 3, "the plot number");
        List<Object> origin = (List<Object>) frame.get("origin");
        check(origin.size() == 3 && ((Number) origin.get(0)).intValue() == 1024 && ((Number) origin.get(2)).intValue() == 0, "the origin array");
        check(Boolean.TRUE.equals(top.get("ok")), "true");
        check(top.containsKey("why") && top.get("why") == null, "null is a present key with no value");

        Map<String, Object> drive = (Map<String, Object>) ((Map<String, Object>) Json.parse("{\"drive\":{\"move\":[1,-0.5,0.25],\"look\":[12.5,-3e1],\"plot\":0,\"owner\":false}}")).get("drive");
        List<Object> move = (List<Object>) drive.get("move");
        check(((Number) move.get(1)).doubleValue() == -0.5 && ((Number) move.get(2)).doubleValue() == 0.25, "decimals and negatives");
        check(((Number) ((List<Object>) drive.get("look")).get(1)).doubleValue() == -30, "an exponent");
        check(Boolean.FALSE.equals(drive.get("owner")), "false");

        check("a \"quoted\" line\nnext \\ A".equals(Json.parse("\"a \\\"quoted\\\" line\\nnext \\\\ \\u0041\"")), "string escapes: " + Json.parse("\"a \\\"quoted\\\" line\\nnext \\\\ \\u0041\""));
        check(((List<Object>) Json.parse(" [ ] ")).isEmpty() && ((Map<String, Object>) Json.parse("{}")).isEmpty(), "empty containers with whitespace");

        for (String bad : new String[] { "", "{", "[1,]", "{\"a\" 1}", "tru", "\"unterminated", "{} x", "[01]", "{\"a\":1,}" }) {
            boolean refused = false;
            try {
                Json.parse(bad);
            } catch (IllegalArgumentException e) {
                refused = true;
            }
            check(refused, "not refused: " + bad);
        }

        String hello = Json.write(Map.of("hello", "redfab-camera", "player", "Steve_7"));
        Map<String, Object> back = (Map<String, Object>) Json.parse(hello);
        check("redfab-camera".equals(back.get("hello")) && "Steve_7".equals(back.get("player")), "the hello round-trips: " + hello);
        check(Json.write("a\"b\\c\n").equals("\"a\\\"b\\\\c\\n\""), "a string is quoted and escaped: " + Json.write("a\"b\\c\n"));
        check(Json.write(List.of(1, 2.5, true)).equals("[1,2.5,true]"), "numbers and booleans write plainly: " + Json.write(List.of(1, 2.5, true)));
        check(Json.write(null).equals("null"), "null writes as null");

        if (failures > 0) {
            System.out.println(failures + " json checks failed");
            System.exit(1);
        }
        System.out.println("json: the backend's messages parse, bad text is refused, the hello writes");
    }
}
