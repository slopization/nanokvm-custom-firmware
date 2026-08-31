// HEVC-over-HTTP transport and browser-side ISO BMFF adapter.
// The box layout follows ISO/IEC 14496-12 and 14496-15.

const be16 = value => new Uint8Array([(value >>> 8) & 255, value & 255]);
const be24 = value => new Uint8Array([(value >>> 16) & 255, (value >>> 8) & 255, value & 255]);
const be32 = value => new Uint8Array([(value >>> 24) & 255, (value >>> 16) & 255, (value >>> 8) & 255, value & 255]);
const be64 = value => concat(be32(Math.floor(value / 0x100000000)), be32(value >>> 0));
const text = value => new TextEncoder().encode(value);
const concat = (...parts) => {
    const size = parts.reduce((total, part) => total + part.length, 0);
    const output = new Uint8Array(size);
    let offset = 0;
    for (const part of parts) {
        output.set(part, offset);
        offset += part.length;
    }
    return output;
};
const box = (type, ...payload) => {
    const body = concat(...payload);
    return concat(be32(body.length + 8), text(type), body);
};
const fullBox = (type, version, flags, ...payload) => box(type, new Uint8Array([version]), be24(flags), ...payload);

export function splitAnnexB(bytes) {
    const starts = [];
    for (let index = 0; index + 3 <= bytes.length;) {
        let prefix = 0;
        if (index + 4 <= bytes.length && bytes[index] === 0 && bytes[index + 1] === 0 && bytes[index + 2] === 0 && bytes[index + 3] === 1) prefix = 4;
        else if (bytes[index] === 0 && bytes[index + 1] === 0 && bytes[index + 2] === 1) prefix = 3;
        if (prefix) {
            starts.push([index, prefix]);
            index += prefix;
        } else index++;
    }
    return starts.map(([start, prefix], position) => bytes.subarray(start + prefix, starts[position + 1]?.[0] ?? bytes.length));
}

const nalType = nal => nal.length >= 2 ? (nal[0] >> 1) & 0x3f : -1;

function rbsp(nal) {
    const output = [];
    let zeros = 0;
    for (const value of nal.subarray(2)) {
        if (zeros >= 2 && value === 3) {
            zeros = 0;
            continue;
        }
        output.push(value);
        zeros = value === 0 ? zeros + 1 : 0;
    }
    return new Uint8Array(output);
}

class Bits {
    constructor(bytes) { this.bytes = bytes; this.offset = 0; }
    read(count) {
        let value = 0;
        for (let index = 0; index < count; index++) {
            if (this.offset >= this.bytes.length * 8) throw new Error("truncated HEVC SPS");
            value = value * 2 + ((this.bytes[this.offset >> 3] >> (7 - (this.offset & 7))) & 1);
            this.offset++;
        }
        return value;
    }
    ue() {
        let zeros = 0;
        while (this.read(1) === 0) {
            if (++zeros > 31) throw new Error("invalid HEVC Exp-Golomb value");
        }
        return (2 ** zeros - 1) + (zeros ? this.read(zeros) : 0);
    }
}

export function parseHevcSps(sps) {
    if (nalType(sps) !== 33) throw new Error("expected HEVC SPS");
    const bits = new Bits(rbsp(sps));
    bits.read(4);
    const maxSubLayersMinus1 = bits.read(3);
    const temporalIdNested = bits.read(1);
    const profileSpace = bits.read(2);
    const tierFlag = bits.read(1);
    const profileIdc = bits.read(5);
    const compatibility = bits.read(32) >>> 0;
    const constraints = new Uint8Array(6);
    for (let index = 0; index < constraints.length; index++) constraints[index] = bits.read(8);
    const levelIdc = bits.read(8);
    const subProfile = [];
    const subLevel = [];
    for (let index = 0; index < maxSubLayersMinus1; index++) {
        subProfile.push(bits.read(1));
        subLevel.push(bits.read(1));
    }
    if (maxSubLayersMinus1) for (let index = maxSubLayersMinus1; index < 8; index++) bits.read(2);
    for (let index = 0; index < maxSubLayersMinus1; index++) {
        if (subProfile[index]) bits.read(88);
        if (subLevel[index]) bits.read(8);
    }
    bits.ue();
    const chromaFormat = bits.ue();
    if (chromaFormat === 3) bits.read(1);
    const width = bits.ue();
    const height = bits.ue();
    if (bits.read(1)) {
        bits.ue(); bits.ue(); bits.ue(); bits.ue();
    }
    const bitDepthLumaMinus8 = bits.ue();
    const bitDepthChromaMinus8 = bits.ue();
    return { maxSubLayersMinus1, temporalIdNested, profileSpace, tierFlag, profileIdc, compatibility, constraints, levelIdc, chromaFormat, width, height, bitDepthLumaMinus8, bitDepthChromaMinus8 };
}

function reverse32(value) {
    let output = 0;
    for (let bit = 0; bit < 32; bit++) output = (output * 2 + ((value >>> bit) & 1)) >>> 0;
    return output >>> 0;
}

export function hevcCodecString(info) {
    const spaces = ["", "A", "B", "C"];
    let value = `hvc1.${spaces[info.profileSpace]}${info.profileIdc}.${reverse32(info.compatibility).toString(16).toUpperCase()}.${info.tierFlag ? "H" : "L"}${info.levelIdc}`;
    let last = info.constraints.length;
    while (last && info.constraints[last - 1] === 0) last--;
    for (const byte of info.constraints.subarray(0, last)) value += `.${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    return value;
}

export function makeHvcc(vps, sps, pps) {
    const info = parseHevcSps(sps);
    const arrays = [vps, sps, pps].map(nal => concat(new Uint8Array([0x80 | nalType(nal)]), be16(1), be16(nal.length), nal));
    const header = concat(
        new Uint8Array([1, (info.profileSpace << 6) | (info.tierFlag << 5) | info.profileIdc]),
        be32(info.compatibility), info.constraints, new Uint8Array([info.levelIdc]),
        be16(0xf000), new Uint8Array([0xfc, 0xfc | info.chromaFormat, 0xf8 | info.bitDepthLumaMinus8, 0xf8 | info.bitDepthChromaMinus8]),
        be16(0), new Uint8Array([(info.maxSubLayersMinus1 + 1) << 3 | info.temporalIdNested << 2 | 3, arrays.length]),
    );
    return { hvcc: concat(header, ...arrays), info, codec: hevcCodecString(info) };
}

export function annexBToSample(bytes) {
    const parts = [];
    for (const nal of splitAnnexB(bytes)) {
        const type = nalType(nal);
        if (type === 32 || type === 33 || type === 34 || type === 35 || type === 38) continue;
        parts.push(be32(nal.length), nal);
    }
    return concat(...parts);
}

const matrix = concat(be32(0x10000), be32(0), be32(0), be32(0), be32(0x10000), be32(0), be32(0), be32(0), be32(0x40000000));

export function makeInitSegment(width, height, timescale, hvcc) {
    const ftyp = box("ftyp", text("iso6"), be32(1), text("iso6"), text("mp41"), text("hvc1"));
    const mvhd = fullBox("mvhd", 0, 0, be32(0), be32(0), be32(timescale), be32(0), be32(0x10000), be16(0x100), be16(0), new Uint8Array(8), matrix, new Uint8Array(24), be32(2));
    const tkhd = fullBox("tkhd", 0, 3, be32(0), be32(0), be32(1), be32(0), be32(0), new Uint8Array(8), be16(0), be16(0), be16(0), be16(0), matrix, be32(width << 16), be32(height << 16));
    const mdhd = fullBox("mdhd", 0, 0, be32(0), be32(0), be32(timescale), be32(0), be16(0x55c4), be16(0));
    const hdlr = fullBox("hdlr", 0, 0, be32(0), text("vide"), new Uint8Array(12), text("VideoHandler\0"));
    const vmhd = fullBox("vmhd", 0, 1, be16(0), be16(0), be16(0), be16(0));
    const url = fullBox("url ", 0, 1);
    const dref = fullBox("dref", 0, 0, be32(1), url);
    const dinf = box("dinf", dref);
    const compressor = new Uint8Array(32);
    const visual = concat(new Uint8Array(6), be16(1), new Uint8Array(16), be16(width), be16(height), be32(0x480000), be32(0x480000), be32(0), be16(1), compressor, be16(24), be16(0xffff), box("hvcC", hvcc));
    const stsd = fullBox("stsd", 0, 0, be32(1), box("hvc1", visual));
    const stbl = box("stbl", stsd, fullBox("stts", 0, 0, be32(0)), fullBox("stsc", 0, 0, be32(0)), fullBox("stsz", 0, 0, be32(0), be32(0)), fullBox("stco", 0, 0, be32(0)));
    const minf = box("minf", vmhd, dinf, stbl);
    const mdia = box("mdia", mdhd, hdlr, minf);
    const trak = box("trak", tkhd, mdia);
    const trex = fullBox("trex", 0, 0, be32(1), be32(1), be32(0), be32(0), be32(0));
    return concat(ftyp, box("moov", mvhd, trak, box("mvex", trex)));
}

export function makeMediaFragment(sequence, timestamp, duration, keyframe, sample) {
    const mfhd = fullBox("mfhd", 0, 0, be32(sequence));
    const tfhd = fullBox("tfhd", 0, 0x020000, be32(1));
    const tfdt = fullBox("tfdt", 1, 0, be64(timestamp));
    const flags = keyframe ? 0x02000000 : 0x01010000;
    const makeMoof = offset => {
        const trun = fullBox("trun", 0, 0x000701, be32(1), be32(offset), be32(duration), be32(sample.length), be32(flags));
        return box("moof", mfhd, box("traf", tfhd, tfdt, trun));
    };
    let moof = makeMoof(0);
    moof = makeMoof(moof.length + 8);
    return concat(moof, box("mdat", sample));
}

class StreamReader {
    constructor(reader) { this.reader = reader; this.buffer = new Uint8Array(0); }
    async take(size) {
        while (this.buffer.length < size) {
            const { value, done } = await this.reader.read();
            if (done) throw new Error("video stream ended");
            this.buffer = concat(this.buffer, value);
        }
        const result = this.buffer.subarray(0, size);
        this.buffer = this.buffer.slice(size);
        return result;
    }
}

async function append(sourceBuffer, bytes) {
    await new Promise((resolve, reject) => {
        const done = () => { cleanup(); resolve(); };
        const failed = () => { cleanup(); reject(new Error("MSE rejected HEVC data")); };
        const cleanup = () => { sourceBuffer.removeEventListener("updateend", done); sourceBuffer.removeEventListener("error", failed); };
        sourceBuffer.addEventListener("updateend", done, { once: true });
        sourceBuffer.addEventListener("error", failed, { once: true });
        sourceBuffer.appendBuffer(bytes);
    });
}

async function removeBuffered(sourceBuffer, start, end) {
    await new Promise((resolve, reject) => {
        const done = () => { cleanup(); resolve(); };
        const failed = () => { cleanup(); reject(new Error("MSE buffer cleanup failed")); };
        const cleanup = () => { sourceBuffer.removeEventListener("updateend", done); sourceBuffer.removeEventListener("error", failed); };
        sourceBuffer.addEventListener("updateend", done, { once: true });
        sourceBuffer.addEventListener("error", failed, { once: true });
        sourceBuffer.remove(start, end);
    });
}

async function openHevcStream(abortSignal, retries = 0) {
    let response;
    for (let attempt = 0;; attempt++) {
        response = await fetch("/api/kvm/stream", { signal: abortSignal });
        if (response.status !== 409 || attempt >= retries) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!response.ok) throw new Error((await response.json()).message);
    const reader = response.body.getReader();
    const input = new StreamReader(reader);
    const preamble = await input.take(24);
    if (new TextDecoder().decode(preamble.subarray(0, 8)) !== "NKHEVC01") throw new Error("invalid video stream preamble");
    const view = new DataView(preamble.buffer, preamble.byteOffset, preamble.byteLength);
    return {
        reader,
        input,
        width: view.getUint32(8),
        height: view.getUint32(12),
        fps: view.getUint32(16),
        timescale: view.getUint32(20),
    };
}

async function readAccessUnit(input) {
    const header = await input.take(12);
    const fields = new DataView(header.buffer, header.byteOffset, header.byteLength);
    return {
        bytes: await input.take(fields.getUint32(0)),
        timestamp: fields.getUint32(4),
        keyframe: (header[8] & 1) !== 0,
    };
}

class NativeHevcUnavailable extends Error {}

async function runNativeHevc(video, status, abortSignal) {
    if (typeof MediaSource === "undefined") throw new NativeHevcUnavailable("Media Source Extensions are unavailable");
    const stream = await openHevcStream(abortSignal);
    const { reader, input, width, height, fps, timescale } = stream;
    let sourceBuffer = null, sequence = 1, firstTimestamp = 0;
    const mediaSource = new MediaSource();
    const objectUrl = URL.createObjectURL(mediaSource);
    video.src = objectUrl;
    try {
        await new Promise(resolve => mediaSource.addEventListener("sourceopen", resolve, { once: true }));
        status.textContent = "Waiting for HDMI keyframe";
        while (!abortSignal.aborted) {
            const frame = await readAccessUnit(input);
            if (!sourceBuffer) {
                if (!frame.keyframe) continue;
                const sets = new Map(splitAnnexB(frame.bytes).map(nal => [nalType(nal), nal]));
                if (![32, 33, 34].every(type => sets.has(type))) continue;
                const config = makeHvcc(sets.get(32), sets.get(33), sets.get(34));
                const mime = `video/mp4; codecs="${config.codec}"`;
                if (!MediaSource.isTypeSupported(mime)) throw new NativeHevcUnavailable(`Native HEVC is unavailable (${config.codec})`);
                try {
                    sourceBuffer = mediaSource.addSourceBuffer(mime);
                } catch (error) {
                    throw new NativeHevcUnavailable(error.message);
                }
                sourceBuffer.mode = "segments";
                await append(sourceBuffer, makeInitSegment(width, height, timescale, config.hvcc));
                firstTimestamp = frame.timestamp;
                status.textContent = `Native HEVC ${width}x${height} ${fps} fps`;
                video.play().catch(() => {});
            }
            const sample = annexBToSample(frame.bytes);
            await append(sourceBuffer, makeMediaFragment(sequence++, (frame.timestamp - firstTimestamp) >>> 0, Math.floor(timescale / fps), frame.keyframe, sample));
            if (video.buffered.length && video.currentTime < video.buffered.end(video.buffered.length - 1) - 0.15) video.currentTime = video.buffered.end(video.buffered.length - 1) - 0.05;
            if (sourceBuffer.buffered.length && sourceBuffer.buffered.start(0) < video.currentTime - 2) await removeBuffered(sourceBuffer, 0, video.currentTime - 1);
        }
    } finally {
        await reader.cancel().catch(() => {});
        video.removeAttribute("src");
        video.load();
        URL.revokeObjectURL(objectUrl);
    }
}

const vertexShader = `#version 300 es
out vec2 texCoord;
void main() {
    vec2 position = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
    texCoord = vec2(position.x, 1.0 - position.y);
}`;

const fragmentShader = `#version 300 es
precision mediump float;
in vec2 texCoord;
uniform sampler2D planeY;
uniform sampler2D planeU;
uniform sampler2D planeV;
uniform mat3 conversion;
uniform vec3 offset;
out vec4 color;
void main() {
    vec3 yuv = vec3(texture(planeY, texCoord).r, texture(planeU, texCoord).r, texture(planeV, texCoord).r);
    color = vec4(clamp(conversion * (yuv + offset), 0.0, 1.0), 1.0);
}`;

function compileShader(gl, type, source) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader));
    return shader;
}

class YuvRenderer {
    constructor(canvas) {
        this.canvas = canvas;
        const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false });
        if (!gl) throw new Error("WebGL2 is required for software HEVC decoding");
        this.gl = gl;
        const program = gl.createProgram();
        gl.attachShader(program, compileShader(gl, gl.VERTEX_SHADER, vertexShader));
        gl.attachShader(program, compileShader(gl, gl.FRAGMENT_SHADER, fragmentShader));
        gl.linkProgram(program);
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
        gl.useProgram(program);
        this.program = program;
        this.textures = [0, 1, 2].map((unit, index) => {
            const texture = gl.createTexture();
            gl.activeTexture(gl.TEXTURE0 + unit);
            gl.bindTexture(gl.TEXTURE_2D, texture);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
            gl.uniform1i(gl.getUniformLocation(program, ["planeY", "planeU", "planeV"][index]), unit);
            return texture;
        });
        this.offset = gl.getUniformLocation(program, "offset");
        this.conversion = gl.getUniformLocation(program, "conversion");
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    }

    draw(frame) {
        const gl = this.gl;
        const width = frame.y.width;
        const height = frame.y.height;
        if (this.canvas.width !== width || this.canvas.height !== height) {
            this.canvas.width = width;
            this.canvas.height = height;
            gl.viewport(0, 0, width, height);
        }
        [frame.y, frame.u, frame.v].forEach((plane, index) => {
            gl.activeTexture(gl.TEXTURE0 + index);
            gl.bindTexture(gl.TEXTURE_2D, this.textures[index]);
            gl.pixelStorei(gl.UNPACK_ROW_LENGTH, plane.stride);
            gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, plane.width, plane.height, 0, gl.RED, gl.UNSIGNED_BYTE, new Uint8Array(plane.bytes));
        });
        gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
        const coefficients = conversionMatrix(frame.matrix, frame.fullRange);
        gl.uniform3fv(this.offset, coefficients.offset);
        gl.uniformMatrix3fv(this.conversion, false, coefficients.matrix);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
    }
}

function conversionMatrix(matrix, fullRange) {
    let kr = 0.2126, kb = 0.0722;
    if (matrix === 5 || matrix === 6) [kr, kb] = [0.299, 0.114];
    else if (matrix === 9 || matrix === 10) [kr, kb] = [0.2627, 0.0593];
    const kg = 1 - kr - kb;
    const yScale = fullRange ? 1 : 255 / 219;
    const cScale = fullRange ? 1 : 255 / 224;
    const rV = 2 * (1 - kr) * cScale;
    const bU = 2 * (1 - kb) * cScale;
    const gU = -2 * kb * (1 - kb) / kg * cScale;
    const gV = -2 * kr * (1 - kr) / kg * cScale;
    return {
        offset: new Float32Array([fullRange ? 0 : -16 / 255, -0.5, -0.5]),
        matrix: new Float32Array([yScale, yScale, yScale, 0, gU, bU, rV, gV, 0]),
    };
}

class SoftwareDecoder {
    constructor(renderer) {
        this.renderer = renderer;
        this.worker = new Worker("/libde265-worker.js", { type: "module" });
        this.waiting = this.newWaiter();
        this.worker.onmessage = event => this.handle(event.data);
        this.worker.onerror = event => this.fail(new Error(event.message || "HEVC decoder worker failed"));
    }

    newWaiter() {
        return new Promise((resolve, reject) => { this.resolve = resolve; this.reject = reject; });
    }

    handle(message) {
        if (message.type === "frame") this.renderer.draw(message);
        else if (message.type === "ready") this.resolve();
        else if (message.type === "error") this.fail(new Error(message.message));
    }

    fail(error) { this.reject(error); }

    async decode(bytes, timestamp) {
        await this.waiting;
        this.waiting = this.newWaiter();
        const accessUnit = bytes.slice().buffer;
        this.worker.postMessage({ type: "decode", accessUnit, timestamp }, [accessUnit]);
    }

    async drain() { await this.waiting; }

    stop() {
        this.fail(new DOMException("decoder stopped", "AbortError"));
        this.worker.terminate();
    }
}

async function runSoftwareHevc(video, canvas, status, abortSignal) {
    const renderer = new YuvRenderer(canvas);
    const decoder = new SoftwareDecoder(renderer);
    const stop = () => decoder.stop();
    abortSignal.addEventListener("abort", stop, { once: true });
    let reader;
    try {
        await decoder.drain();
        const stream = await openHevcStream(abortSignal, 10);
        reader = stream.reader;
        video.classList.add("hidden");
        canvas.classList.remove("hidden");
        status.textContent = `Software HEVC ${stream.width}x${stream.height} ${stream.fps} fps`;
        let started = false;
        while (!abortSignal.aborted) {
            const frame = await readAccessUnit(stream.input);
            if (!started && !frame.keyframe) continue;
            started = true;
            await decoder.decode(frame.bytes, frame.timestamp);
        }
        await decoder.drain();
    } finally {
        abortSignal.removeEventListener("abort", stop);
        decoder.stop();
        await reader?.cancel().catch(() => {});
    }
}

async function runHevc(video, canvas, status, abortSignal) {
    video.classList.remove("hidden");
    canvas.classList.add("hidden");
    try {
        await runNativeHevc(video, status, abortSignal);
    } catch (error) {
        if (!(error instanceof NativeHevcUnavailable) || abortSignal.aborted) throw error;
        status.textContent = `${error.message}; loading software decoder`;
        await runSoftwareHevc(video, canvas, status, abortSignal);
    }
}

let controller = null;
function init() {
    const start = document.getElementById("kvmStart");
    const stop = document.getElementById("kvmStop");
    const video = document.getElementById("kvmVideo");
    const canvas = document.getElementById("kvmCanvas");
    const status = document.getElementById("kvmStatus");
    if (!start || !video) return;
    start.onclick = async () => {
        controller?.abort();
        controller = new AbortController();
        start.disabled = true;
        stop.disabled = false;
        try { await runHevc(video, canvas, status, controller.signal); }
        catch (error) { if (!controller.signal.aborted) status.textContent = error.message; }
        finally { start.disabled = false; stop.disabled = true; }
    };
    stop.onclick = () => controller?.abort();
}

if (typeof document !== "undefined") init();
