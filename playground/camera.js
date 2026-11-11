// The camera tab: the user's own game client, streamed from the relay on the
// box and played here over WebRTC (WHEP: one offer posted, one answer back)
// while the tab is open and the page visible, since every viewer costs
// bandwidth. Two modes: Auto view, where the backend frames the plots and
// orbits; and Manual control, taken over the plot's live socket
// (server/camera.mjs decides who drives), where the keys make a move vector and
// a drag on the video a look, sent ten times a second.

const WHEP = '/camera/camera/whep';
const INPUT_HZ = 10;
const LOOK_DEGREES_PER_PIXEL = 0.3;
// WebRTC's "disconnected" often passes on its own; only a stream gone this long is restarted.
const DISCONNECTED_GRACE_MS = 5000;
const RETRY_MS = 3000;

// Key -> [axis, sign] in the character's own frame: x right, y up, z forward.
const KEYS = { w: [2, 1], s: [2, -1], d: [0, 1], a: [0, -1], ' ': [1, 1], shift: [1, -1], arrowup: [2, 1], arrowdown: [2, -1], arrowright: [0, 1], arrowleft: [0, -1] };

export function createCameraPanel({ panel, video, status, auto, manual, turn }) {
    let feature = null; // the backend's camera status, or null when it has none
    let send = () => false; // sends on the plot's live socket; false when there is none
    let canSend = false;
    let myPlot = null;
    let pc = null;
    let resource = null; // the WHEP resource to DELETE when stopping
    let playing = false;
    let shown = false; // the tab is open
    let pageHidden = false;
    let driving = false;
    let queued = null; // our place in the queue for a turn, while waiting
    let note = null; // a passing message (an error, a turn that ended), shown until the next change
    let countdown = null;
    let poll = null;
    let ticker = null;
    let retry = null;
    let grace = null;
    const pressed = new Set();
    let look = [0, 0];

    const wanted = () => shown && !pageHidden && feature?.connected && feature.streaming !== false;

    /// The plot the camera is on, and whether it is the one this page holds.
    const watching = () => (feature?.plot === null || feature?.plot === undefined ? null : { plot: feature.plot, yours: feature.plot === myPlot });

    function statusText() {
        if (!feature?.connected) return 'Camera offline';
        if (feature.streaming === false) return 'Camera offline: the stream is not up';
        if (driving) return 'You are driving: W A S D to move, space and shift for up and down, drag the video to look, Esc to stop';
        if (note) return note;
        if (!playing) return 'Connecting…';
        const on = watching();
        return `Live from the game${on ? `, plot ${on.plot}${on.yours ? ' (yours)' : ''}` : ''}`;
    }

    const render = () => {
        panel.classList.toggle('playing', playing);
        if (!feature) return;
        status.textContent = statusText();
        auto.setAttribute('aria-pressed', String(!driving));
        manual.setAttribute('aria-pressed', String(driving));
        manual.disabled = !feature.connected || !canSend;
        manual.title = canSend ? 'Drive the camera yourself: W A S D, space and shift, drag to look' : 'Run a design in the game to drive the camera';
        if (driving) return;
        if (queued !== null) turn.textContent = `waiting for a turn, ${queued} in line`;
        else turn.textContent = feature.driver ? 'someone else is driving' : '';
    };

    /// Plays when it should and stops when it should not; called on every change.
    function settle() {
        if (wanted() && !playing && !pc && !retry) start();
        else if (!wanted() && (playing || pc)) stop();
        render();
    }

    async function refresh() {
        try {
            const reply = await fetch('/api/status');
            const body = reply.ok ? await reply.json() : null;
            feature = body?.camera ?? null;
        } catch {
            feature = null;
        }
        settle();
    }

    // --- the video

    async function start() {
        if (playing || pc) return;
        note = null;
        const mine = new RTCPeerConnection();
        pc = mine;
        mine.addTransceiver('video', { direction: 'recvonly' });
        mine.addTransceiver('audio', { direction: 'recvonly' });
        mine.ontrack = (e) => { video.srcObject = e.streams[0]; };
        mine.onconnectionstatechange = () => {
            if (pc !== mine) return;
            const state = mine.connectionState;
            if (state === 'connected' && grace) {
                clearTimeout(grace);
                grace = null;
            } else if (state === 'disconnected' && !grace) {
                grace = setTimeout(() => { grace = null; if (pc === mine && mine.connectionState !== 'connected') restart('the stream dropped'); }, DISCONNECTED_GRACE_MS);
            } else if (state === 'failed') {
                restart('the stream failed');
            }
        };
        try {
            const offer = await mine.createOffer();
            await mine.setLocalDescription(offer);
            await new Promise((ok) => {
                if (mine.iceGatheringState === 'complete') return ok();
                const done = () => { if (mine.iceGatheringState === 'complete') { mine.removeEventListener('icegatheringstatechange', done); ok(); } };
                mine.addEventListener('icegatheringstatechange', done);
                setTimeout(ok, 2000); // enough candidates by then
            });
            if (pc !== mine) return; // stopped while gathering: ask the relay for nothing
            const reply = await fetch(WHEP, { method: 'POST', headers: { 'content-type': 'application/sdp' }, body: mine.localDescription.sdp });
            if (!reply.ok) throw new Error(`the relay answered ${reply.status}`);
            if (pc !== mine) return; // stopped meanwhile
            resource = reply.headers.get('location');
            await mine.setRemoteDescription({ type: 'answer', sdp: await reply.text() });
            playing = true;
            render();
        } catch (err) {
            if (pc === mine) restart(err.message);
        }
    }

    function stop() {
        if (grace) clearTimeout(grace);
        grace = null;
        if (retry) clearTimeout(retry);
        retry = null;
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
    }

    /// Stops, says why, and tries again shortly while the tab still wants the stream.
    function restart(why) {
        stop();
        note = `Camera: ${why}; retrying`;
        render();
        retry = setTimeout(() => { retry = null; settle(); }, RETRY_MS);
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

    /// Takes the camera, or gives it back when driving.
    function take() {
        const message = { drive: driving ? 'release' : 'take' };
        send(message);
        return message;
    }

    /// A drive answer from the backend, off the plot's socket.
    function receive(m) {
        if (m.drive === 'yours') {
            driving = true;
            queued = null;
            note = null;
            if (countdown) clearInterval(countdown);
            if (typeof m.seconds === 'number') {
                let left = m.seconds;
                turn.textContent = `${left} s left`;
                countdown = setInterval(() => { left -= 1; turn.textContent = `${Math.max(left, 0)} s left`; }, 1000);
            } else {
                turn.textContent = 'yours while the plot is yours';
            }
            if (!ticker) ticker = setInterval(input, 1000 / INPUT_HZ);
        } else if (m.drive === 'queued') {
            queued = m.position;
        } else if (m.drive === 'over') {
            driving = false;
            queued = null;
            pressed.clear();
            look = [0, 0];
            if (countdown) clearInterval(countdown);
            countdown = null;
            if (ticker) clearInterval(ticker);
            ticker = null;
            turn.textContent = '';
            note = `Back to auto view: ${m.why}`;
        } else if (m.ok === false && /drive|driving|camera/.test(m.error ?? '')) {
            note = `Camera: ${m.error}`;
        }
        render();
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

    auto.addEventListener('click', () => { if (driving) take(); });
    manual.addEventListener('click', () => { if (!driving && queued === null) take(); });

    /// The page went hidden or came back: the stream stops to save the relay's bandwidth, and resumes.
    function hidden(isHidden) {
        pageHidden = isHidden;
        settle();
    }
    document.addEventListener('visibilitychange', () => hidden(document.visibilityState === 'hidden'));

    return {
        /// The backend's camera status from the page's first status fetch: null means no camera here.
        init(camera) {
            feature = camera;
            settle();
            if (camera && !poll) poll = setInterval(refresh, 5000);
        },
        /// Whether the backend has a camera at all (the page shows the tab only then).
        get available() { return feature !== null; },
        /// The tab opened or closed: the stream plays only while it is open.
        show(on) {
            shown = on;
            settle();
        },
        /// The plot's socket to send drive messages on (null when there is none), and the plot held.
        attach(sender, plot) {
            send = sender ?? (() => false);
            canSend = sender !== null;
            myPlot = plot;
            if (!sender) queued = null;
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
        get shown() { return shown; },
        get watching() { return watching(); },
        get status() { return feature; },
    };
}
