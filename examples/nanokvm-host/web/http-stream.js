const STREAM_MAGIC = "NKHEVC01";
const PREAMBLE_SIZE = 24;
const FRAME_HEADER_SIZE = 12;
const MAX_WIDTH = 8192;
const MAX_HEIGHT = 4320;
const MAX_PIXEL_COUNT = 4096 * 2160;
const MAX_FPS = 240;
const MAX_ACCESS_UNIT_SIZE = 32 * 1024 * 1024;

export class ByteQueue {
    constructor(reader) {
        this.reader = reader;
        this.chunks = [];
        this.offset = 0;
        this.length = 0;
    }

    async take(size) {
        if (!Number.isSafeInteger(size) || size < 0 || size > MAX_ACCESS_UNIT_SIZE) {
            throw new Error("invalid video stream item size");
        }
        while (this.length < size) {
            const { value, done } = await this.reader.read();
            if (done) throw new Error("video stream ended");
            if (value?.length) {
                this.chunks.push(value);
                this.length += value.length;
            }
        }
        const first = this.chunks[0];
        const available = first.length - this.offset;
        if (available >= size) {
            const result = first.subarray(this.offset, this.offset + size);
            this.offset += size;
            this.length -= size;
            if (this.offset === first.length) {
                this.chunks.shift();
                this.offset = 0;
            }
            return result;
        }
        const result = new Uint8Array(size);
        let written = 0;
        while (written < size) {
            const chunk = this.chunks[0];
            const count = Math.min(size - written, chunk.length - this.offset);
            result.set(chunk.subarray(this.offset, this.offset + count), written);
            this.offset += count;
            written += count;
            this.length -= count;
            if (this.offset === chunk.length) {
                this.chunks.shift();
                this.offset = 0;
            }
        }
        return result;
    }
}

function abortableDelay(milliseconds, signal) {
    return new Promise((resolve, reject) => {
        if (signal.aborted) return reject(signal.reason);
        const timer = setTimeout(done, milliseconds);
        signal.addEventListener("abort", aborted, { once: true });
        function cleanup() {
            clearTimeout(timer);
            signal.removeEventListener("abort", aborted);
        }
        function done() { cleanup(); resolve(); }
        function aborted() { cleanup(); reject(signal.reason); }
    });
}

function validateMetadata(width, height, fps, timescale) {
    if (width < 1 || width > MAX_WIDTH || height < 1 || height > MAX_HEIGHT || width * height > MAX_PIXEL_COUNT) {
        throw new Error("video stream dimensions exceed protocol limits");
    }
    if (fps < 1 || fps > MAX_FPS) throw new Error("video stream frame rate exceeds protocol limits");
    if (timescale < 1 || timescale > 1_000_000_000) throw new Error("invalid video stream timescale");
}

async function responseError(response) {
    const fallback = `video request failed (${response.status})`;
    const contentType = response.headers.get("content-type") || "";
    if (contentType.includes("application/json")) {
        try { return (await response.json()).message || fallback; } catch { return fallback; }
    }
    try { return (await response.text()) || fallback; } catch { return fallback; }
}

export async function openHevcStream(abortSignal, retries = 0) {
    let response;
    for (let attempt = 0;; attempt++) {
        response = await fetch("/api/kvm/stream", { signal: abortSignal });
        if (response.status !== 409 || attempt >= retries) break;
        await response.body?.cancel().catch(() => {});
        await abortableDelay(100, abortSignal);
    }
    if (!response.ok) throw new Error(await responseError(response));
    if (!response.body) throw new Error("video response has no body");
    const reader = response.body.getReader();
    const input = new ByteQueue(reader);
    try {
        const preamble = await input.take(PREAMBLE_SIZE);
        if (new TextDecoder().decode(preamble.subarray(0, 8)) !== STREAM_MAGIC) {
            throw new Error("invalid video stream preamble");
        }
        const view = new DataView(preamble.buffer, preamble.byteOffset, preamble.byteLength);
        const stream = {
            reader,
            input,
            width: view.getUint32(8),
            height: view.getUint32(12),
            fps: view.getUint32(16),
            timescale: view.getUint32(20),
        };
        validateMetadata(stream.width, stream.height, stream.fps, stream.timescale);
        return stream;
    } catch (error) {
        await reader.cancel().catch(() => {});
        throw error;
    }
}

export async function readAccessUnit(input) {
    const header = await input.take(FRAME_HEADER_SIZE);
    const fields = new DataView(header.buffer, header.byteOffset, header.byteLength);
    const size = fields.getUint32(0);
    const flags = header[8];
    if (size < 1 || size > MAX_ACCESS_UNIT_SIZE) throw new Error("HEVC access unit exceeds protocol limit");
    if ((flags & ~1) !== 0 || header[9] !== 0 || header[10] !== 0 || header[11] !== 0) {
        throw new Error("unsupported video frame flags");
    }
    return {
        bytes: await input.take(size),
        timestamp: fields.getUint32(4),
        keyframe: (flags & 1) !== 0,
    };
}

export class TimestampUnwrapper {
    constructor() {
        this.previous = null;
        this.epoch = 0;
    }

    unwrap(timestamp) {
        if (this.previous !== null && timestamp < this.previous) {
            if (this.previous - timestamp <= 0x80000000) {
                throw new Error("video timestamp moved backward");
            }
            this.epoch += 0x100000000;
        }
        this.previous = timestamp;
        return this.epoch + timestamp;
    }
}
