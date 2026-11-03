// JSON, enough for the backend's messages and the mod's own: objects as
// LinkedHashMap, arrays as ArrayList, strings, numbers as Double, booleans,
// null; anything else is refused with what was wrong and where. No library,
// so the mod depends on nothing but the game and the loader.
package dev.redfab.camera;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class Json {
    private final String text;
    private int at;

    private Json(String text) {
        this.text = text;
    }

    /// The value the text holds; IllegalArgumentException when it is not JSON.
    public static Object parse(String text) {
        Json p = new Json(text);
        p.skip();
        Object value = p.value();
        p.skip();
        if (p.at != text.length()) throw p.error("text after the value");
        return value;
    }

    private IllegalArgumentException error(String what) {
        return new IllegalArgumentException("not JSON at " + at + ": " + what);
    }

    private void skip() {
        while (at < text.length() && Character.isWhitespace(text.charAt(at))) at++;
    }

    private char peek() {
        if (at >= text.length()) throw error("the text ends");
        return text.charAt(at);
    }

    private void expect(char c) {
        if (peek() != c) throw error("expected " + c);
        at++;
    }

    private Object value() {
        char c = peek();
        switch (c) {
            case '{': return object();
            case '[': return array();
            case '"': return string();
            case 't': literal("true"); return Boolean.TRUE;
            case 'f': literal("false"); return Boolean.FALSE;
            case 'n': literal("null"); return null;
            default:
                if (c == '-' || (c >= '0' && c <= '9')) return number();
                throw error("unexpected " + c);
        }
    }

    private void literal(String word) {
        if (!text.startsWith(word, at)) throw error("expected " + word);
        at += word.length();
    }

    private Map<String, Object> object() {
        expect('{');
        Map<String, Object> map = new LinkedHashMap<>();
        skip();
        if (peek() == '}') {
            at++;
            return map;
        }
        while (true) {
            skip();
            if (peek() != '"') throw error("expected a key");
            String key = string();
            skip();
            expect(':');
            skip();
            map.put(key, value());
            skip();
            char c = peek();
            at++;
            if (c == '}') return map;
            if (c != ',') throw error("expected , or }");
        }
    }

    private List<Object> array() {
        expect('[');
        List<Object> list = new ArrayList<>();
        skip();
        if (peek() == ']') {
            at++;
            return list;
        }
        while (true) {
            skip();
            list.add(value());
            skip();
            char c = peek();
            at++;
            if (c == ']') return list;
            if (c != ',') throw error("expected , or ]");
        }
    }

    private String string() {
        expect('"');
        StringBuilder sb = new StringBuilder();
        while (true) {
            char c = peek();
            at++;
            if (c == '"') return sb.toString();
            if (c == '\\') {
                char e = peek();
                at++;
                switch (e) {
                    case '"': sb.append('"'); break;
                    case '\\': sb.append('\\'); break;
                    case '/': sb.append('/'); break;
                    case 'b': sb.append('\b'); break;
                    case 'f': sb.append('\f'); break;
                    case 'n': sb.append('\n'); break;
                    case 'r': sb.append('\r'); break;
                    case 't': sb.append('\t'); break;
                    case 'u':
                        if (at + 4 > text.length()) throw error("a short \\u escape");
                        try {
                            sb.append((char) Integer.parseInt(text.substring(at, at + 4), 16));
                        } catch (NumberFormatException ex) {
                            throw error("a bad \\u escape");
                        }
                        at += 4;
                        break;
                    default: throw error("a bad escape \\" + e);
                }
            } else if (c < 0x20) {
                throw error("a control character in a string");
            } else {
                sb.append(c);
            }
        }
    }

    private boolean digit() {
        return at < text.length() && text.charAt(at) >= '0' && text.charAt(at) <= '9';
    }

    private Double number() {
        int start = at;
        if (peek() == '-') at++;
        if (peek() == '0') {
            at++;
        } else {
            if (!digit()) throw error("expected a digit");
            while (digit()) at++;
        }
        if (at < text.length() && text.charAt(at) == '.') {
            at++;
            if (!digit()) throw error("expected a digit after the point");
            while (digit()) at++;
        }
        if (at < text.length() && (text.charAt(at) == 'e' || text.charAt(at) == 'E')) {
            at++;
            if (at < text.length() && (text.charAt(at) == '+' || text.charAt(at) == '-')) at++;
            if (!digit()) throw error("expected a digit in the exponent");
            while (digit()) at++;
        }
        return Double.valueOf(text.substring(start, at));
    }

    /// The JSON text for a map, list, string, number, boolean or null.
    public static String write(Object value) {
        StringBuilder sb = new StringBuilder();
        write(sb, value);
        return sb.toString();
    }

    private static void write(StringBuilder sb, Object value) {
        if (value == null) {
            sb.append("null");
        } else if (value instanceof String s) {
            quote(sb, s);
        } else if (value instanceof Boolean b) {
            sb.append(b ? "true" : "false");
        } else if (value instanceof Number n) {
            double d = n.doubleValue();
            if (d == Math.rint(d) && !Double.isInfinite(d) && Math.abs(d) < 1e15) sb.append((long) d);
            else sb.append(d);
        } else if (value instanceof Map<?, ?> m) {
            sb.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> e : m.entrySet()) {
                if (!first) sb.append(',');
                first = false;
                quote(sb, String.valueOf(e.getKey()));
                sb.append(':');
                write(sb, e.getValue());
            }
            sb.append('}');
        } else if (value instanceof Iterable<?> list) {
            sb.append('[');
            boolean first = true;
            for (Object item : list) {
                if (!first) sb.append(',');
                first = false;
                write(sb, item);
            }
            sb.append(']');
        } else {
            quote(sb, String.valueOf(value));
        }
    }

    private static void quote(StringBuilder sb, String s) {
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
            }
        }
        sb.append('"');
    }
}
