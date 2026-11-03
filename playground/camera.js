// The camera panel: the user's own game client, streamed from the relay on
// the box and played here over WebRTC (WHEP: one offer posted, one answer
// back), on click and stopped while the page is hidden, since every viewer
// costs bandwidth. Taking the camera and driving it go over the plot's live
// socket (server/camera.mjs decides who drives): the keys make a move
// vector, a drag on the video a look, sent ten times a second while driving.

const WHEP = '/camera/camera/whep';
const INPUT_HZ = 10;
const LOOK_DEGREES_PER_PIXEL = 0.3;

// Key -> [axis, sign] in the character's own frame: x right, y up, z forward.
const KEYS = { w: [2, 1], s: [2, -1], d: [0, 1], a: [0, -1], ' ': [1, 1], shift: [1, -1], arrowup: [2, 1], arrowdown: [2, -1], arrowright: [0, 1], arrowleft: [0, -1] };

export function createCameraPanel({ panel, video, status, play, drive, turn, on, log = () => {} }) {
    let feature = null; // the backend's camera status, or null when it has none
    let send = () => false; // sends on the plot's live socket; false when there is none
    let canSend = false;
    let myPlot = null;
    let pc = null;
    let resource = null; // the WHEP resource to DELETE when stopping
    let playing = false;
    let resume = false; // play again when the page is visible again
    let driving = false;
    let countdown = null;
    let poll = null;
    let ticker = null;
    const pressed = new Set();
    let look = [0, 0];

    const say = (text) => { status.textContent = text; };

    const render = () => {
        panel.hidden = feature === null;
        panel.classList.toggle('playing', playing);
        if (!feature) return;
        if (!feature.connected) say('camera off: the camera is not connected');
        else if (feature.streaming === false) say('camera off: the stream is not up');
        else if (playing) say('camera live: playing');
        else say('camera up: press play');
        play.disabled = !(feature.connected && feature.streaming !== false) && !playing;
        play.textContent = playing ? 'Stop' : 'Play';
        on.hidden = !(myPlot !== null && feature.plot === myPlot);
        drive.disabled = !feature.connected || !canSend;
        if (!driving && !turn.dataset.queued) turn.textContent = feature.driver ? (feature.queue ? `someone drives, ${feature.queue} waiting` : 'someone drives') : '';
    };

    async function refresh() {
        try {
            const reply = await fetch('/api/status');
            const body = reply.ok ? await reply.json() : null;
            feature = body?.camera ?? null;
        } catch {
            feature = null;
        }
        render();
    }

    // --- the video

    async function start() {
        if (playing || pc) return;
        pc = new RTCPeerConnection();
        pc.addTransceiver('video', { direction: 'recvonly' });
        pc.addTransceiver('audio', { direction: 'recvonly' });
        pc.ontrack = (e) => { video.srcObject = e.streams[0]; };
        pc.onconnectionstatechange = () => {
            if (pc && (pc.connectionState === 'failed' || pc.connectionState === 'disconnected')) stop('the connection dropped');
        };
        try {
            const offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            await new Promise((ok) => {
                if (pc.iceGatheringState === 'complete') return ok();
                const done = () => { if (pc.iceGatheringState === 'complete') { pc.removeEventListener('icegatheringstatechange', done); ok(); } };
                pc.addEventListener('icegatheringstatechange', done);
                setTimeout(ok, 2000); // enough candidates by then
            });
            const reply = await fetch(WHEP, { method: 'POST', headers: { 'content-type': 'application/sdp' }, body: pc.localDescription.sdp });
            if (!reply.ok) throw new Error(`the relay answered ${reply.status}`);
            resource = reply.headers.get('location');
            await pc.setRemoteDescription({ type: 'answer', sdp: await reply.text() });
            playing = true;
            render();
        } catch (err) {
            stop(err.message);
        }
    }

    function stop(why = null) {
        if (pc) {
            pc.close();
            pc = null;
        }
        if (resource) {
            fetch(resource, { method: 'DELETE' }).catch(() => {});
            resource = null;
        }
        video.srcObject = null;
        playing = false;
        render();
        if (why) say(`camera: ${why}`);
    }

    // --- driving

    function input() {
        const move = [0, 0, 0];
        for (const key of pressed) {
            const [axis, sign] = KEYS[key];
            move[axis] = Math.max(-1, Math.min(1, move[axis] + sign));
        }
        const turned = look[0] !== 0 || look[1] !== 0;
        if (!move.some((v) => v !== 0) && !turned) return null;
        const message = { drive: { move, look: [Math.round(look[0] * 100) / 100, Math.round(look[1] * 100) / 100] } };
        look = [0, 0];
        if (driving) send(message);
        return message;
    }

    function take() {
        const message = { drive: driving ? 'release' : 'take' };
        send(message);
        return message;
    }

    /// A drive answer from the backend, off the plot's socket.
    function receive(m) {
        if (m.drive === 'yours') {
            driving = true;
            delete turn.dataset.queued;
            drive.textContent = 'Give it back';
            say('camera: yours, drive with W A S D, space and shift, drag to look');
            if (countdown) clearInterval(countdown);
            if (typeof m.seconds === 'number') {
                let left = m.seconds;
                turn.textContent = `${left} s`;
                countdown = setInterval(() => { left -= 1; turn.textContent = `${Math.max(left, 0)} s`; }, 1000);
            } else {
                turn.textContent = 'yours while the plot is yours';
            }
            if (!ticker) ticker = setInterval(input, 1000 / INPUT_HZ);
        } else if (m.drive === 'queued') {
            turn.dataset.queued = '1';
            turn.textContent = `in the queue at ${m.position}`;
        } else if (m.drive === 'over') {
            driving = false;
            delete turn.dataset.queued;
            pressed.clear();
            look = [0, 0];
            if (countdown) clearInterval(countdown);
            countdown = null;
            if (ticker) clearInterval(ticker);
            ticker = null;
            drive.textContent = 'Take the camera';
            turn.textContent = '';
            say(`camera: over (${m.why})`);
        } else if (m.ok === false && /drive|driving|camera/.test(m.error ?? '')) {
            say(`camera: ${m.error}`);
        }
    }

    function press(key, down) {
        const k = key.toLowerCase();
        if (!(k in KEYS)) return false;
        if (down) pressed.add(k);
        else pressed.delete(k);
        return true;
    }

    const typing = (e) => ['TEXTAREA', 'INPUT', 'SELECT'].includes(e.target?.tagName);
    document.addEventListener('keydown', (e) => {
        if (!driving || typing(e)) return;
        if (e.key === 'Escape') { take(); return; }
        if (press(e.key, true)) e.preventDefault();
    });
    document.addEventListener('keyup', (e) => {
        if (!driving || typing(e)) return;
        press(e.key, false);
    });
    let dragging = null;
    video.addEventListener('pointerdown', (e) => { if (driving) { dragging = [e.clientX, e.clientY]; video.setPointerCapture(e.pointerId); } });
    video.addEventListener('pointermove', (e) => {
        if (!dragging) return;
        look = [look[0] + (e.clientX - dragging[0]) * LOOK_DEGREES_PER_PIXEL, look[1] + (e.clientY - dragging[1]) * LOOK_DEGREES_PER_PIXEL];
        dragging = [e.clientX, e.clientY];
    });
    video.addEventListener('pointerup', () => { dragging = null; });

    play.addEventListener('click', () => (playing ? stop() : start()));
    drive.addEventListener('click', take);

    /// The page went hidden or came back: stop to save the relay's bandwidth, resume after.
    function hidden(isHidden) {
        if (isHidden) {
            resume = playing;
            if (playing) stop();
        } else if (resume) {
            resume = false;
            start();
        }
    }
    document.addEventListener('visibilitychange', () => hidden(document.visibilityState === 'hidden'));

    return {
        /// The backend's camera status from the page's first status fetch: null means no camera here.
        init(camera) {
            feature = camera;
            render();
            if (camera && !poll) poll = setInterval(refresh, 5000);
        },
        /// The plot's socket to send drive messages on (null when there is none), and the plot held.
        attach(sender, plot) {
            send = sender ?? (() => false);
            canSend = sender !== null;
            myPlot = plot;
            if (driving && !sender) receive({ drive: 'over', why: 'the plot went' });
            render();
        },
        refresh,
        receive,
        take,
        press,
        look: (dx, dy) => { look = [look[0] + dx, look[1] + dy]; },
        input,
        hidden,
        get playing() { return playing; },
        get driving() { return driving; },
        get status() { return feature; },
    };
}
