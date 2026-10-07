// Minecraft's RCON protocol, the way harness/rcon.py does it. Packets are
// little-endian: int32 length, int32 id, int32 type, body, two NUL bytes. Two
// server behaviours shape the client (both observed on 26.3): replies longer
// than 4096 bytes are split with no end marker, so a sentinel packet of an
// unknown type follows every command and its answer marks the end; and the
// server wants one request per socket read, so the sentinel goes out only once
// the command's first reply packet has arrived.

import { createConnection } from 'node:net';

const AUTH = 3;
const COMMAND = 2;
const SENTINEL = 0x7f;
const MAX_COMMAND_BYTES = 1446;

export class RconError extends Error {}

export class Rcon {
    static connect(host, port, password, { timeout = 30_000 } = {}) {
        return new Promise((resolve, reject) => {
            const socket = createConnection({ host, port });
            const timer = setTimeout(() => {
                socket.destroy();
                reject(new RconError(`RCON did not connect within ${timeout} ms`));
            }, timeout);
            socket.once('error', (e) => {
                clearTimeout(timer);
                reject(e);
            });
            socket.once('connect', () => {
                const rcon = new Rcon(socket);
                rcon.auth(password).then(() => {
                    clearTimeout(timer);
                    resolve(rcon);
                }, (e) => {
                    clearTimeout(timer);
                    socket.destroy();
                    reject(e);
                });
            });
        });
    }

    constructor(socket) {
        this.socket = socket;
        this.socket.setNoDelay(true);
        this.buffer = Buffer.alloc(0);
        this.nextId = 1;
        this.queue = [];
        this.current = null;
        this.authWaiting = null;
        this.commandTimeout = 30_000;
        socket.on('data', (chunk) => this.onData(chunk));
        socket.on('close', () => this.fail(new RconError('the RCON connection closed')));
        socket.on('error', (e) => this.fail(e instanceof Error ? e : new RconError(String(e))));
    }

    takeId() {
        this.nextId += 1;
        return this.nextId;
    }

    send(id, type, body) {
        const text = Buffer.from(body, 'utf8');
        const packet = Buffer.alloc(4 + 8 + text.length + 2);
        packet.writeInt32LE(8 + text.length + 2, 0);
        packet.writeInt32LE(id, 4);
        packet.writeInt32LE(type, 8);
        text.copy(packet, 12);
        this.socket.write(packet);
    }

    auth(password) {
        return new Promise((resolve, reject) => {
            this.authWaiting = { resolve, reject };
            this.send(this.takeId(), AUTH, password);
        });
    }

    /// Runs one command and resolves with its full reply text. Commands run one at a time.
    command(text) {
        if (Buffer.byteLength(text, 'utf8') > MAX_COMMAND_BYTES) {
            return Promise.reject(new RconError(`command is longer than ${MAX_COMMAND_BYTES} bytes`));
        }
        return new Promise((resolve, reject) => {
            this.queue.push({ text, resolve, reject });
            this.pump();
        });
    }

    pump() {
        if (this.current || !this.queue.length) return;
        const job = this.queue.shift();
        this.current = { ...job, cmdId: this.takeId(), endId: this.takeId(), parts: [], sentinelSent: false };
        this.current.timer = setTimeout(() => {
            this.fail(new RconError(`no reply to ${JSON.stringify(job.text)} within ${this.commandTimeout} ms`));
            this.socket.destroy();
        }, this.commandTimeout);
        this.send(this.current.cmdId, COMMAND, job.text);
    }

    onData(chunk) {
        this.buffer = Buffer.concat([this.buffer, chunk]);
        for (;;) {
            if (this.buffer.length < 4) return;
            const length = this.buffer.readInt32LE(0);
            if (this.buffer.length < 4 + length) return;
            const id = this.buffer.readInt32LE(4);
            const body = this.buffer.subarray(12, 4 + length - 2).toString('utf8');
            this.buffer = this.buffer.subarray(4 + length);
            this.onPacket(id, body);
        }
    }

    onPacket(id, body) {
        if (this.authWaiting) {
            const waiting = this.authWaiting;
            this.authWaiting = null;
            if (id === -1) waiting.reject(new RconError('RCON authentication failed'));
            else waiting.resolve();
            return;
        }
        const current = this.current;
        if (!current) return;
        if (id === current.endId) {
            clearTimeout(current.timer);
            this.current = null;
            current.resolve(current.parts.join(''));
            this.pump();
        } else if (id === current.cmdId) {
            current.parts.push(body);
            if (!current.sentinelSent) {
                current.sentinelSent = true;
                this.send(current.endId, SENTINEL, '');
            }
        } else {
            this.fail(new RconError(`unexpected RCON reply id ${id} for ${JSON.stringify(current.text)}`));
        }
    }

    fail(error) {
        if (this.authWaiting) {
            this.authWaiting.reject(error);
            this.authWaiting = null;
        }
        if (this.current) {
            clearTimeout(this.current.timer);
            this.current.reject(error);
            this.current = null;
        }
        for (const job of this.queue.splice(0)) job.reject(error);
    }

    close() {
        this.socket.destroy();
    }
}
