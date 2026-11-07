let module;
let decoder;

async function initializeDecoder() {
    const threaded = self.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined";
    const base = threaded ? "/vendor/libde265-threaded" : "/vendor/libde265";
    const { default: initialize } = await import(`${base}/libde265.mjs`);
    module = await initialize({
        locateFile: name => `${base}/${name}`,
    });
    decoder = new module.Decoder();

    const cpus = navigator.hardwareConcurrency || 1;
    let threads = 0;
    let threadError = "";
    if (threaded) {
        // The threaded WASM build has a fixed pool of four pthreads.
        threads = Math.max(1, Math.min(4, cpus));
        try {
            const error = decoder.startWorkerThreads(threads);
            if (!module.isOk(error)) {
                threadError = `startWorkerThreads failed (${error})`;
                threads = 0;
            }
        } catch (error) {
            threads = 0;
            threadError = error.message || String(error);
        }
    }
    if (threadError) {
        postMessage({ type: "threads_error", message: threadError });
    }
    postMessage({
        type: "ready",
        threads,
        threaded,
        isolated: self.crossOriginIsolated,
        shared: typeof SharedArrayBuffer !== "undefined",
        cpus,
        threadError,
    });
}

function copyPlane(image, channel) {
    const plane = image.getImagePlane(channel);
    const bytes = new Uint8Array(plane.bytes);
    return {
        width: plane.width,
        height: plane.height,
        stride: plane.stride,
        bytes: bytes.slice().buffer,
    };
}

function decode(accessUnit, timestamp) {
    const input = new Uint8Array(accessUnit);
    let error = decoder.pushData(input, BigInt(timestamp));
    if (!module.isOk(error)) throw new Error(module.getErrorText(error));
    decoder.pushEndOfFrame();

    let more = true;
    while (more) {
        const result = decoder.decode();
        more = result.more;
        if (!module.isOk(result.error)) {
            if (result.error === module.Error.ERROR_WAITING_FOR_INPUT_DATA) break;
            throw new Error(module.getErrorText(result.error));
        }
        for (let image = decoder.getNextPicture(); image; image = decoder.getNextPicture()) {
            if (image.chromaFormat !== 1 || image.getBitsPerPixel(0) !== 8) {
                image.delete();
                throw new Error("software decoder supports 8-bit 4:2:0 HEVC only");
            }
            const y = copyPlane(image, 0);
            const u = copyPlane(image, 1);
            const v = copyPlane(image, 2);
            const frame = {
                type: "frame",
                timestamp: Number(image.pts),
                fullRange: image.isFullRange,
                matrix: image.matrixCoefficients,
                y,
                u,
                v,
            };
            image.delete();
            postMessage(frame, [y.bytes, u.bytes, v.bytes]);
        }
    }
    postMessage({ type: "ready" });
}

onmessage = event => {
    if (event.data.type === "decode") {
        try {
            decode(event.data.accessUnit, event.data.timestamp);
        } catch (error) {
            postMessage({ type: "error", message: error.message });
        }
    } else if (event.data.type === "reset" && decoder) {
        decoder.reset();
        postMessage({ type: "ready" });
    }
};

initializeDecoder().catch(error => postMessage({ type: "error", message: error.message }));
