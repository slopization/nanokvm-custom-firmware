import { openHevcStream, readAccessUnit } from "./http-stream.js";

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
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
        throw new Error(gl.getShaderInfoLog(shader));
    }
    return shader;
}

function conversionMatrix(matrix, fullRange) {
    let kr = 0.2126;
    let kb = 0.0722;
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
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
            throw new Error(gl.getProgramInfoLog(program));
        }
        gl.useProgram(program);
        this.textures = ["planeY", "planeU", "planeV"].map((uniform, unit) => {
            const texture = gl.createTexture();
            gl.activeTexture(gl.TEXTURE0 + unit);
            gl.bindTexture(gl.TEXTURE_2D, texture);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
            gl.uniform1i(gl.getUniformLocation(program, uniform), unit);
            return texture;
        });
        this.planeSizes = [null, null, null];
        this.offset = gl.getUniformLocation(program, "offset");
        this.conversion = gl.getUniformLocation(program, "conversion");
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    }

    draw(frame) {
        const gl = this.gl;
        if (this.canvas.width !== frame.y.width || this.canvas.height !== frame.y.height) {
            this.canvas.width = frame.y.width;
            this.canvas.height = frame.y.height;
            gl.viewport(0, 0, frame.y.width, frame.y.height);
        }
        [frame.y, frame.u, frame.v].forEach((plane, index) => {
            gl.activeTexture(gl.TEXTURE0 + index);
            gl.bindTexture(gl.TEXTURE_2D, this.textures[index]);
            const dimensions = `${plane.width}x${plane.height}`;
            if (this.planeSizes[index] !== dimensions) {
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, plane.width, plane.height, 0, gl.RED, gl.UNSIGNED_BYTE, null);
                this.planeSizes[index] = dimensions;
            }
            gl.pixelStorei(gl.UNPACK_ROW_LENGTH, plane.stride);
            gl.texSubImage2D(
                gl.TEXTURE_2D,
                0,
                0,
                0,
                plane.width,
                plane.height,
                gl.RED,
                gl.UNSIGNED_BYTE,
                new Uint8Array(plane.bytes),
            );
        });
        gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
        const coefficients = conversionMatrix(frame.matrix, frame.fullRange);
        gl.uniform3fv(this.offset, coefficients.offset);
        gl.uniformMatrix3fv(this.conversion, false, coefficients.matrix);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
    }
}

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    promise.catch(() => {});
    return { promise, resolve, reject };
}

class SoftwareDecoder {
    constructor(renderer) {
        this.renderer = renderer;
        this.worker = new Worker("/libde265-worker.js", { type: "module" });
        this.ready = deferred();
        this.failure = null;
        this.threads = 0;
        this.threadInfo = { threads: 0, isolated: false, shared: false, cpus: 0, threadError: "" };
        this.worker.onmessage = event => this.handle(event.data);
        this.worker.onerror = event => this.fail(new Error(event.message || "HEVC decoder worker failed"));
    }

    handle(message) {
        if (message.type === "frame") {
            try {
                this.renderer.draw(message);
            } catch (error) {
                this.fail(error);
            }
        } else if (message.type === "ready") {
            this.threads = message.threads || 0;
            this.threadInfo = message;
            this.ready.resolve();
        } else if (message.type === "threads_error") {
            this.threadInfo.threadError = message.message;
            console.warn("HEVC decoder threads unavailable:", message.message);
        } else if (message.type === "error") {
            this.fail(new Error(message.message));
        }
    }

    fail(error) {
        if (this.failure) return;
        this.failure = error;
        this.ready.reject(error);
        this.worker.terminate();
    }

    async decode(bytes, timestamp) {
        await this.ready.promise;
        if (this.failure) throw this.failure;
        this.ready = deferred();
        const accessUnit = bytes.slice().buffer;
        this.worker.postMessage({ type: "decode", accessUnit, timestamp }, [accessUnit]);
    }

    async drain() {
        await this.ready.promise;
        if (this.failure) throw this.failure;
    }

    stop(reason = new DOMException("decoder stopped", "AbortError")) {
        this.fail(reason || new DOMException("decoder stopped", "AbortError"));
    }
}

export async function runSoftwareHevc(video, canvas, status, abortSignal) {
    const decoder = new SoftwareDecoder(new YuvRenderer(canvas));
    const stop = () => decoder.stop(abortSignal.reason);
    abortSignal.addEventListener("abort", stop, { once: true });
    let reader;
    try {
        await decoder.drain();
        const stream = await openHevcStream(abortSignal, 10);
        reader = stream.reader;
        video.classList.add("hidden");
        canvas.classList.remove("hidden");
        const info = decoder.threadInfo;
        let threads = info.threads > 1 ? `${info.threads} threads` : "1 thread";
        if (info.threads <= 1) {
            if (!info.isolated) threads += " (not cross-origin isolated)";
            else if (!info.shared) threads += " (no SharedArrayBuffer)";
            else if (info.threadError) threads += ` (${info.threadError})`;
            else if (info.cpus <= 1) threads += " (1 CPU)";
        }
        status.textContent = `Software HEVC ${stream.width}x${stream.height} ${stream.fps} fps, ${threads}`;
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
