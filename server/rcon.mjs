// Minecraft's RCON protocol, the way harness/rcon.py does it. Packets are
// little-endian: int32 length, int32 id, int32 type, body, two NUL bytes. Two
// server behaviours shape the client (both observed on 26.3): replies longer
// than 4096 bytes are split with no end marker, so a sentinel packet of an
// unknown type follows every command and its answer marks the end; and the
// server wants one request per socket read, so the sentinel goes out only once
// the command's first reply packet has arrived.
//
// The connection is not for life: when the socket closes or a command gets no
// reply in time, every command in hand is rejected, and the next command dials
// and authenticates again. `up` says whether a socket is open right now.
//
// One connection per server, not one per purpose: the vanilla server runs every
// RCON client's commands through one shared console source (DedicatedServer
// .runCommand clears it, runs the command on the server thread and reads it
// back, with a thread per client and no lock), so two connections mix and lose
// each other's replies. A command that must not wait behind the queue (a clock
// edge) is sent `urgent`: it goes out next, ahead of what is queued.

import { createConnection } from 'node:net';

const AUTH = 3;
const COMMAND = 2;
const SENTINEL = 0x7f;
const MAX_COMMAND_BYTES = 1446;

export class RconError extends Error {}

export class Rcon {
    /// Dials and authenticates; rejects when the server is not there or the password is wrong.
    static async connect(host, port, password, options = {}) {
        const rcon = new Rcon(host, port, password, options);
        await rcon.dial();
        return rcon;
    }

    constructor(host, port, password, { timeout = 30_000, commandTimeout = 30_000, log = () => {} } = {}) {
        this.host = host;
        this.port = port;
        this.password = password;
        this.connectTimeout = timeout;
        this.commandTimeout = commandTimeout;
        this.log = log;
        this.socket = null;
        this.dialing = null;
        this.closed = false;
        this.reconnects = -1; // the first dial is not a reconnect
        this.buffer = Buffer.alloc(0);
        this.nextId = 1;
        this.queue = [];
        this.current = null;
        this.authWaiting = null;
    }

    /// Whether a socket is open and authenticated.
    get up() {
        return this.socket !== null && !this.socket.destroyed;
    }

    /// Opens the socket and authenticates, once at a time: concurrent callers share the attempt.
    dial() {
        if (this.closed) return Promise.reject(new RconError('the RCON client is closed'));
        if (this.up) return Promise.resolve();
        if (this.dialing) return this.dialing;
        this.dialing = this.open().finally(() => {
            this.dialing = null;
        });
        return this.dialing;
    }

    open() {
        return new Promise((resolve, reject) => {
            const socket = createConnection({ host: this.host, port: this.port });
            socket.setNoDelay(true);
            const timer = setTimeout(() => {
                socket.destroy();
                reject(new RconError(`RCON did not connect within ${this.connectTimeout} ms`));
            }, this.connectTimeout);
            socket.once('error', (e) => {
                clearTimeout(timer);
                reject(e instanceof Error ? e : new RconError(String(e)));
            });
            socket.once('connect', () => {
                this.socket = socket;
                this.buffer = Buffer.alloc(0);
                socket.on('data', (chunk) => this.onData(chunk));
                socket.on('close', () => this.drop(socket, new RconError('the RCON connection closed')));
                socket.on('error', (e) => this.drop(socket, e instanceof Error ? e : new RconError(String(e))));
                this.auth().then(() => {
                    clearTimeout(timer);
                    this.reconnects += 1;
                    resolve();
                }, (e) => {
                    clearTimeout(timer);
                    socket.destroy();
                    reject(e);
                });
            });
        });
    }

    /// The socket went away: the command in flight and the queue are rejected, explicitly.
    drop(socket, error) {
        if (socket !== this.socket) return;
        this.socket = null;
        socket.destroy();
        this.fail(error);
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

    auth() {
        return new Promise((resolve, reject) => {
            this.authWaiting = { resolve, reject };
            this.send(this.takeId(), AUTH, this.password);
        });
    }

    /// Runs one command and resolves with its full reply text. Commands run one
    /// at a time, in order, an `urgent` one ahead of the queue; with no socket
    /// open, the first command dials.
    command(text, { urgent = false } = {}) {
        if (Buffer.byteLength(text, 'utf8') > MAX_COMMAND_BYTES) {
            return Promise.reject(new RconError(`command is longer than ${MAX_COMMAND_BYTES} bytes`));
        }
        if (this.closed) return Promise.reject(new RconError('the RCON client is closed'));
        return new Promise((resolve, reject) => {
            if (urgent) this.queue.unshift({ text, resolve, reject });
            else this.queue.push({ text, resolve, reject });
            this.pump();
        });
    }

    pump() {
        if (this.current || !this.queue.length) return;
        if (!this.up) {
            this.dial().then(() => this.pump(), (err) => {
                this.log(`RCON: ${err.message}`);
                this.fail(err);
            });
            return;
        }
        const job = this.queue.shift();
        this.current = { ...job, cmdId: this.takeId(), endId: this.takeId(), parts: [], sentinelSent: false };
        this.current.timer = setTimeout(() => {
            const socket = this.socket;
            this.drop(socket, new RconError(`no reply to ${JSON.stringify(job.text)} within ${this.commandTimeout} ms`));
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
            this.drop(this.socket, new RconError(`unexpected RCON reply id ${id} for ${JSON.stringify(current.text)}`));
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

    /// Closes for good: no command runs after this.
    close() {
        this.closed = true;
        const socket = this.socket;
        this.socket = null;
        socket?.destroy();
        this.fail(new RconError('the RCON client is closed'));
    }
}
