// The 3D view of a built design: every block an instance of a unit cube, scaled
// by kind, coloured by kind and state. The simulator's changes recolour
// instances in place; nothing is ever added or removed after a build, since a
// redstone circuit only changes block states. A frame is drawn only when
// something changed (a recolour, the camera moving, a resize), and none while
// the view is hidden: drawing every frame kept a laptop's GPU busy for a still
// picture, and behind the camera tab for no picture at all.

import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';

// Kinds that share one instanced mesh, with their box size and vertical offset.
const SHAPES = {
    block:    { size: [1, 1, 1],       lift: 0 },
    dust:     { size: [0.9, 0.08, 0.9], lift: 0 },
    torch:    { size: [0.25, 0.7, 0.25], lift: 0 },
    repeater: { size: [1, 0.18, 1],    lift: 0 },
    lever:    { size: [0.3, 0.5, 0.3], lift: 0 },
};

function kindOf(state) {
    const name = state.split('[')[0];
    switch (name) {
        case 'minecraft:redstone_wire': return 'dust';
        case 'minecraft:redstone_torch':
        case 'minecraft:redstone_wall_torch': return 'torch';
        case 'minecraft:repeater': return 'repeater';
        case 'minecraft:lever': return 'lever';
        default: return 'block';
    }
}

function colourOf(state) {
    const name = state.split('[')[0];
    const prop = (key) => (state.match(new RegExp(`${key}=([a-z0-9_]+)`)) || [])[1];
    switch (name) {
        case 'minecraft:glass': return new THREE.Color(0x9fd3e6);
        case 'minecraft:stone': return new THREE.Color(0x8a8a8a);
        case 'minecraft:redstone_block': return new THREE.Color(0xd81e1e);
        case 'minecraft:redstone_lamp': return prop('lit') === 'true' ? new THREE.Color(0xffd34d) : new THREE.Color(0x5a4420);
        case 'minecraft:redstone_wire': {
            const power = Number(prop('power') || 0);
            return new THREE.Color().setHSL(0, 0.9, 0.18 + 0.5 * (power / 15));
        }
        case 'minecraft:redstone_torch':
        case 'minecraft:redstone_wall_torch': return prop('lit') === 'false' ? new THREE.Color(0x4a2a1a) : new THREE.Color(0xff9a3c);
        case 'minecraft:repeater': return prop('powered') === 'true' ? new THREE.Color(0xd8655a) : new THREE.Color(0x8d7f7a);
        case 'minecraft:lever': return new THREE.Color(0x6b5a3e);
        default: return new THREE.Color(0xff00ff);
    }
}

export class View {
    constructor(container) {
        this.container = container;
        this.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
        this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
        container.appendChild(this.renderer.domElement);
        this.scene = new THREE.Scene();
        this.scene.background = new THREE.Color(0xdcdcdc); // a blank world's grey, so the red routing reads as in the game
        this.camera = new THREE.PerspectiveCamera(45, 1, 0.1, 5000);
        this.controls = new OrbitControls(this.camera, this.renderer.domElement);
        this.controls.enableDamping = true;
        this.scene.add(new THREE.AmbientLight(0xffffff, 0.9));
        const sun = new THREE.DirectionalLight(0xffffff, 1.2);
        sun.position.set(0.4, 1, 0.3);
        this.scene.add(sun);
        this.meshes = {};
        this.instances = new Map(); // "x,y,z" -> { kind, index }
        this.onClick = null;
        this.raycaster = new THREE.Raycaster();
        this.pointer = new THREE.Vector2();
        this.pressed = null;
        this.renderer.domElement.addEventListener('pointerdown', (e) => { this.pressed = [e.clientX, e.clientY]; });
        this.renderer.domElement.addEventListener('pointerup', (e) => this.pick(e));
        this.visible = true;
        this.frame = null;
        this.frames = 0; // drawn so far (for the checks: a still view draws none)
        this.controls.addEventListener('change', () => this.requestRender());
        new ResizeObserver(() => this.resize()).observe(container);
        this.resize();
    }

    resize() {
        const { clientWidth: w, clientHeight: h } = this.container;
        if (w === 0 || h === 0) return;
        this.renderer.setSize(w, h, false);
        this.camera.aspect = w / h;
        this.camera.updateProjectionMatrix();
        this.requestRender();
    }

    /// Asks for one frame; the damping of a drag asks for more until it settles.
    requestRender() {
        if (this.frame === null && this.visible) this.frame = requestAnimationFrame(() => this.draw());
    }

    draw() {
        this.frame = null;
        const moving = this.controls.update();
        this.renderer.render(this.scene, this.camera);
        this.frames += 1;
        if (moving) this.requestRender();
    }

    /// Shown or hidden (behind the camera tab): hidden, nothing is drawn.
    setVisible(on) {
        this.visible = on;
        if (on) this.resize();
        else if (this.frame !== null) {
            cancelAnimationFrame(this.frame);
            this.frame = null;
        }
    }

    /// Removes the design on show, leaving an empty scene.
    clear() {
        for (const mesh of Object.values(this.meshes)) {
            this.scene.remove(mesh);
            mesh.geometry.dispose();
        }
        this.meshes = {};
        this.instances.clear();
        this.requestRender();
    }

    /// Shows a design: its blocks, with the camera framing them.
    show(design) {
        this.clear();
        const byKind = {};
        for (const [pos, state] of Object.entries(design.blocks)) {
            (byKind[kindOf(state)] ??= []).push([pos, state]);
        }
        const dummy = new THREE.Object3D();
        for (const [kind, list] of Object.entries(byKind)) {
            const { size, lift } = SHAPES[kind];
            const geometry = new THREE.BoxGeometry(...size);
            const material = new THREE.MeshLambertMaterial({ transparent: kind === 'block', opacity: 1 });
            const mesh = new THREE.InstancedMesh(geometry, material, list.length);
            list.forEach(([pos, state], i) => {
                const [x, y, z] = pos.split(',').map(Number);
                dummy.position.set(x + 0.5, y + size[1] / 2 + lift, z + 0.5);
                dummy.updateMatrix();
                mesh.setMatrixAt(i, dummy.matrix);
                mesh.setColorAt(i, colourOf(state));
                this.instances.set(pos, { kind, index: i });
            });
            mesh.instanceMatrix.needsUpdate = true;
            mesh.instanceColor.needsUpdate = true;
            this.scene.add(mesh);
            this.meshes[kind] = mesh;
        }
        // Glass is see-through so the wiring on it stays visible.
        if (this.meshes.block) this.meshes.block.material.opacity = 0.55;
        const [sx, sy, sz] = design.size;
        this.controls.target.set(sx / 2, 1, sz / 2);
        const span = Math.max(sx, sz);
        this.camera.position.set(sx / 2 + span * 0.25, span * 0.6 + 8, sz / 2 + span * 0.7);
        this.camera.near = 0.1;
        this.camera.far = span * 10 + 100;
        this.camera.updateProjectionMatrix();
        this.controls.update();
        this.requestRender();
    }

    /// Recolours the blocks that changed state.
    update(changes) {
        const touched = new Set();
        for (const [pos, state] of changes) {
            const at = this.instances.get(pos);
            if (!at) continue;
            this.meshes[at.kind].setColorAt(at.index, colourOf(state));
            touched.add(at.kind);
        }
        for (const kind of touched) this.meshes[kind].instanceColor.needsUpdate = true;
        if (touched.size) this.requestRender();
    }

    /// The colour a block is drawn in, as a hex string (for the checks).
    colourAt(pos) {
        const at = this.instances.get(pos);
        if (!at) return null;
        const colour = new THREE.Color();
        this.meshes[at.kind].getColorAt(at.index, colour);
        return colour.getHexString();
    }

    /// The block under a click, if the pointer did not drag.
    pick(event) {
        if (!this.pressed || !this.onClick) return;
        const moved = Math.hypot(event.clientX - this.pressed[0], event.clientY - this.pressed[1]) > 4;
        this.pressed = null;
        if (moved) return;
        const rect = this.renderer.domElement.getBoundingClientRect();
        this.pointer.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -((event.clientY - rect.top) / rect.height) * 2 + 1);
        this.raycaster.setFromCamera(this.pointer, this.camera);
        const hits = this.raycaster.intersectObjects(Object.values(this.meshes));
        for (const hit of hits) {
            const point = hit.point.clone().sub(hit.face.normal.clone().multiplyScalar(0.01));
            const pos = `${Math.floor(point.x)},${Math.floor(point.y)},${Math.floor(point.z)}`;
            if (this.instances.has(pos)) {
                this.onClick(pos);
                return;
            }
        }
    }

    /// Where a block's centre is on screen, in page pixels (for the checks).
    screenPosition(pos) {
        const [x, y, z] = pos.split(',').map(Number);
        this.camera.updateMatrixWorld(); // the camera may have moved since the last frame
        const v = new THREE.Vector3(x + 0.5, y + 0.5, z + 0.5).project(this.camera);
        const rect = this.renderer.domElement.getBoundingClientRect();
        return { x: rect.left + ((v.x + 1) / 2) * rect.width, y: rect.top + ((1 - v.y) / 2) * rect.height };
    }
}
