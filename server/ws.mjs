// A WebSocket server for the browser: enough of RFC 6455 to accept an upgrade,
// read small masked text frames, answer pings, and push text frames back.

import { createHash } from 'node:crypto';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

/// Completes the handshake on an `upgrade` event. Returns the connection, or
/// null (with the socket closed) when the request is not a WebSocket one.
export function acceptWebSocket(req, socket, head, { maxMessage = 65_536 } = {}) {
    const key = req.headers['sec-websocket-key'];
    if (!key || String(req.headers.upgrade || '').toLowerCase() !== 'websocket') {
        socket.write('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n');
        socket.destroy();
        return null;
    }
    const accept = createHash('sha1').update(key + GUID).digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    socket.setNoDelay(true);
    const connection = new Connection(socket, maxMessage);
    if (head && head.length) connection.onData(head);
    return connection;
}

class Connection {
    constructor(socket, maxMessage) {
        this.socket = socket;
        this.maxMessage = maxMessage;
        this.buffer = Buffer.alloc(0);
        this.closed = false;
        this.onMessage = null;
        this.onClose = null;
        socket.on('data', (chunk) => this.onData(chunk));
        socket.on('close', () => this.finish());
        socket.on('error', () => this.finish());
    }

    onData(chunk) {
        if (this.closed) return;
        this.buffer = Buffer.concat([this.buffer, chunk]);
        for (;;) {
            if (this.buffer.length < 2) return;
            const first = this.buffer[0];
            const second = this.buffer[1];
            const fin = (first & 0x80) !== 0;
            const opcode = first & 0x0f;
            const masked = (second & 0x80) !== 0;
            let length = second & 0x7f;
            let offset = 2;
            if (length === 126) {
                if (this.buffer.length < 4) return;
                length = this.buffer.readUInt16BE(2);
                offset = 4;
            } else if (length === 127) {
                if (this.buffer.length < 10) return;
                length = Number(this.buffer.readBigUInt64BE(2));
                offset = 10;
            }
            if (length > this.maxMessage) {
                this.close(1009, 'message too big');
                return;
            }
            if (!masked) {
                this.close(1002, 'client frames must be masked');
                return;
            }
            const total = offset + 4 + length;
            if (this.buffer.length < total) return;
            const mask = this.buffer.subarray(offset, offset + 4);
            const payload = Buffer.from(this.buffer.subarray(offset + 4, total));
            for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
            this.buffer = this.buffer.subarray(total);
            switch (opcode) {
                case 0x1:
                    if (!fin) {
                        this.close(1003, 'fragmented messages are not supported');
                        return;
                    }
                    if (this.onMessage) this.onMessage(payload.toString('utf8'));
                    break;
                case 0x8:
                    this.frame(0x8, payload.subarray(0, 2));
                    this.socket.end();
                    this.finish();
                    return;
                case 0x9:
                    this.frame(0xa, payload);
                    break;
                case 0xa:
                    break;
                default:
                    this.close(1003, 'only text messages are accepted');
                    return;
            }
        }
    }

    frame(opcode, payload) {
        if (this.socket.destroyed) return;
        const n = payload.length;
        let header;
        if (n < 126) {
            header = Buffer.from([0x80 | opcode, n]);
        } else if (n < 65_536) {
            header = Buffer.alloc(4);
            header[0] = 0x80 | opcode;
            header[1] = 126;
            header.writeUInt16BE(n, 2);
        } else {
            header = Buffer.alloc(10);
            header[0] = 0x80 | opcode;
            header[1] = 127;
            header.writeBigUInt64BE(BigInt(n), 2);
        }
        this.socket.write(Buffer.concat([header, payload]));
    }

    send(text) {
        if (!this.closed) this.frame(0x1, Buffer.from(text, 'utf8'));
    }

    close(code = 1000, reason = '') {
        if (this.closed) return;
        const body = Buffer.alloc(2 + Buffer.byteLength(reason));
        body.writeUInt16BE(code, 0);
        body.write(reason, 2);
        this.frame(0x8, body);
        this.socket.end();
        this.finish();
    }

    finish() {
        if (this.closed) return;
        this.closed = true;
        if (this.onClose) this.onClose();
    }
}
