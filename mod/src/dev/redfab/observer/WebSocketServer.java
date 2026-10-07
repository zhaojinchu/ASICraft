// A WebSocket server that only ever sends: enough of RFC 6455 to accept a
// browser or a test client on localhost and push text frames to it. No
// dependencies, so the mod needs nothing but the loader.
package dev.redfab.observer;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Base64;
import java.util.List;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.function.Supplier;

public final class WebSocketServer {
    private static final String GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

    private final int port;
    private final Supplier<String> hello;
    private final List<Client> clients = new ArrayList<>();

    public WebSocketServer(int port, Supplier<String> hello) {
        this.port = port;
        this.hello = hello;
    }

    public void start() {
        Thread acceptor = new Thread(this::accept, "redfab-observer-accept");
        acceptor.setDaemon(true);
        acceptor.start();
    }

    private void accept() {
        try (ServerSocket listener = new ServerSocket(port, 16, InetAddress.getLoopbackAddress())) {
            while (true) {
                Socket socket = listener.accept();
                socket.setTcpNoDelay(true);
                Thread handler = new Thread(() -> serve(socket), "redfab-observer-client");
                handler.setDaemon(true);
                handler.start();
            }
        } catch (IOException e) {
            System.out.println("[redfab-observer] cannot listen on " + port + ": " + e.getMessage());
        }
    }

    private void serve(Socket socket) {
        Client client = null;
        try {
            InputStream in = socket.getInputStream();
            OutputStream out = socket.getOutputStream();
            String key = handshake(in);
            if (key == null) {
                socket.close();
                return;
            }
            MessageDigest sha1 = MessageDigest.getInstance("SHA-1");
            String accept = Base64.getEncoder().encodeToString(sha1.digest((key + GUID).getBytes(StandardCharsets.US_ASCII)));
            out.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                + "Sec-WebSocket-Accept: " + accept + "\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            out.flush();
            client = new Client(socket, out);
            synchronized (clients) {
                clients.add(client);
            }
            client.send(hello.get());
            // Read the client's frames: answer pings, stop on close, ignore the rest.
            while (true) {
                int first = in.read();
                if (first < 0) break;
                int opcode = first & 0x0F;
                int second = in.read();
                if (second < 0) break;
                long length = second & 0x7F;
                if (length == 126) length = readBigEndian(in, 2);
                else if (length == 127) length = readBigEndian(in, 8);
                byte[] mask = new byte[4];
                if ((second & 0x80) != 0 && in.readNBytes(mask, 0, 4) < 4) break;
                byte[] payload = in.readNBytes((int) Math.min(length, 1 << 20));
                for (int i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
                if (opcode == 0x8) break;
                if (opcode == 0x9) client.frame(0xA, payload);
            }
        } catch (Exception e) {
            // A client that goes away is not an error worth logging.
        } finally {
            if (client != null) {
                synchronized (clients) {
                    clients.remove(client);
                }
                client.close();
            }
            try {
                socket.close();
            } catch (IOException ignored) {
            }
        }
    }

    private static long readBigEndian(InputStream in, int bytes) throws IOException {
        long value = 0;
        for (int i = 0; i < bytes; i++) {
            int b = in.read();
            if (b < 0) throw new IOException("short frame");
            value = (value << 8) | b;
        }
        return value;
    }

    /// Reads the HTTP upgrade request; returns the Sec-WebSocket-Key, or null.
    private static String handshake(InputStream in) throws IOException {
        StringBuilder request = new StringBuilder();
        int b;
        while ((b = in.read()) >= 0) {
            request.append((char) b);
            if (request.length() > 8192) return null;
            if (request.toString().endsWith("\r\n\r\n")) break;
        }
        String key = null;
        boolean upgrade = false;
        for (String line : request.toString().split("\r\n")) {
            int colon = line.indexOf(':');
            if (colon < 0) continue;
            String name = line.substring(0, colon).trim().toLowerCase();
            String value = line.substring(colon + 1).trim();
            if (name.equals("sec-websocket-key")) key = value;
            if (name.equals("upgrade") && value.equalsIgnoreCase("websocket")) upgrade = true;
        }
        return upgrade ? key : null;
    }

    public void broadcast(String text) {
        List<Client> snapshot;
        synchronized (clients) {
            snapshot = new ArrayList<>(clients);
        }
        for (Client client : snapshot) {
            client.send(text);
        }
    }

    /// One client: frames go out from its own thread, so the server thread never waits on a socket.
    private static final class Client {
        private final Socket socket;
        private final OutputStream out;
        private final LinkedBlockingQueue<byte[]> queue = new LinkedBlockingQueue<>(4096);
        private volatile boolean open = true;

        Client(Socket socket, OutputStream out) {
            this.socket = socket;
            this.out = out;
            Thread writer = new Thread(this::drain, "redfab-observer-writer");
            writer.setDaemon(true);
            writer.start();
        }

        void send(String text) {
            frame(0x1, text.getBytes(StandardCharsets.UTF_8));
        }

        void frame(int opcode, byte[] payload) {
            byte[] header;
            int n = payload.length;
            if (n < 126) {
                header = new byte[] {(byte) (0x80 | opcode), (byte) n};
            } else if (n < 65536) {
                header = new byte[] {(byte) (0x80 | opcode), 126, (byte) (n >> 8), (byte) n};
            } else {
                header = new byte[10];
                header[0] = (byte) (0x80 | opcode);
                header[1] = 127;
                for (int i = 0; i < 8; i++) header[2 + i] = (byte) (((long) n) >> (8 * (7 - i)));
            }
            byte[] frame = new byte[header.length + n];
            System.arraycopy(header, 0, frame, 0, header.length);
            System.arraycopy(payload, 0, frame, header.length, n);
            if (!queue.offer(frame)) {
                close(); // a client that cannot keep up is dropped rather than slowing the server
            }
        }

        private void drain() {
            try {
                while (open) {
                    byte[] frame = queue.take();
                    out.write(frame);
                    out.flush();
                }
            } catch (Exception e) {
                close();
            }
        }

        void close() {
            open = false;
            try {
                socket.close();
            } catch (IOException ignored) {
            }
        }
    }
}
