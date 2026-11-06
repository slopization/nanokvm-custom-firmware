import { annexBToSample, makeHvcc, nalType, splitAnnexB } from "./hevc.js";
import { makeInitSegment, makeMediaFragment } from "./isobmff.js";
import { TimestampUnwrapper, openHevcStream, readAccessUnit } from "./http-stream.js";

export class NativeHevcUnavailable extends Error {}

function abortError(signal) {
    return signal.reason || new DOMException("operation aborted", "AbortError");
}

function waitForEvent(target, successEvent, failureEvent, signal, action) {
    return new Promise((resolve, reject) => {
        if (signal.aborted) return reject(abortError(signal));
        const succeeded = () => { cleanup(); resolve(); };
        const failed = () => { cleanup(); reject(new Error(`media operation failed (${failureEvent})`)); };
        const aborted = () => { cleanup(); reject(abortError(signal)); };
        const cleanup = () => {
            target.removeEventListener(successEvent, succeeded);
            if (failureEvent) target.removeEventListener(failureEvent, failed);
            signal.removeEventListener("abort", aborted);
        };
        target.addEventListener(successEvent, succeeded, { once: true });
        if (failureEvent) target.addEventListener(failureEvent, failed, { once: true });
        signal.addEventListener("abort", aborted, { once: true });
        try {
            action?.();
        } catch (error) {
            cleanup();
            reject(error);
        }
    });
}

const append = (sourceBuffer, bytes, signal) =>
    waitForEvent(sourceBuffer, "updateend", "error", signal, () => sourceBuffer.appendBuffer(bytes));

const removeBuffered = (sourceBuffer, start, end, signal) =>
    waitForEvent(sourceBuffer, "updateend", "error", signal, () => sourceBuffer.remove(start, end));

export async function runNativeHevc(video, status, abortSignal) {
    if (typeof MediaSource === "undefined") {
        throw new NativeHevcUnavailable("Media Source Extensions are unavailable");
    }
    const stream = await openHevcStream(abortSignal);
    if (stream.gop === 0) {
        await stream.reader.cancel().catch(() => {});
        throw new NativeHevcUnavailable("MSE requires a periodic keyframe; using software decoder");
    }
    const { reader, input, width, height, fps, timescale } = stream;
    const timestamps = new TimestampUnwrapper();
    const frameDuration = Math.max(1, Math.floor(timescale / Math.max(1, fps)));
    let sourceBuffer = null;
    let sequence = 1;
    let firstTimestamp;
    const keyframes = [];
    const mediaSource = new MediaSource();
    const objectUrl = URL.createObjectURL(mediaSource);
    video.src = objectUrl;
    try {
        await waitForEvent(mediaSource, "sourceopen", "error", abortSignal);
        status.textContent = "Waiting for HDMI keyframe";
        while (!abortSignal.aborted) {
            const frame = await readAccessUnit(input);
            const absoluteTimestamp = timestamps.unwrap(frame.timestamp);
            if (!sourceBuffer) {
                if (!frame.keyframe) continue;
                const sets = new Map(splitAnnexB(frame.bytes).map(nal => [nalType(nal), nal]));
                if (![32, 33, 34].every(type => sets.has(type))) continue;
                const config = makeHvcc(sets.get(32), sets.get(33), sets.get(34));
                const mime = `video/mp4; codecs="${config.codec}"`;
                if (!MediaSource.isTypeSupported(mime)) {
                    throw new NativeHevcUnavailable(`Native HEVC is unavailable (${config.codec})`);
                }
                try {
                    sourceBuffer = mediaSource.addSourceBuffer(mime);
                    sourceBuffer.mode = "segments";
                    await append(sourceBuffer, makeInitSegment(width, height, timescale, config.hvcc), abortSignal);
                } catch (error) {
                    if (abortSignal.aborted) throw error;
                    throw new NativeHevcUnavailable(error.message);
                }
                firstTimestamp = absoluteTimestamp;
                status.textContent = `Native HEVC ${width}x${height} ${fps} fps`;
                video.play().catch(() => {});
            }

            const current = {
                bytes: frame.bytes,
                timestamp: absoluteTimestamp - firstTimestamp,
                keyframe: frame.keyframe,
            };
            if (current.keyframe) keyframes.push(current.timestamp);

            // The server emits one access unit per fixed frame interval. Append
            // immediately with a constant duration instead of holding the
            // previous frame until the next one arrives.
            await append(
                sourceBuffer,
                makeMediaFragment(
                    sequence++,
                    current.timestamp,
                    frameDuration,
                    current.keyframe,
                    annexBToSample(current.bytes),
                ),
                abortSignal,
            );

            if (video.buffered.length) {
                const liveEdge = video.buffered.end(video.buffered.length - 1);
                const target = liveEdge - 0.05;
                const bufferedStart = sourceBuffer.buffered.start(0);
                // Seeking resets the decoder. Do not jump to the live edge
                // if the nearest keyframe is far behind: a long GOP would
                // force a full GOP re-decode and look like a freeze. Stay
                // on the continuous decode path in that case.
                let nearestKeyframe = null;
                for (const timestamp of keyframes) {
                    if (timestamp >= bufferedStart && timestamp <= target) nearestKeyframe = timestamp;
                }
                const canSeek = nearestKeyframe !== null && target - nearestKeyframe <= 1.0;
                if (stream.seek && canSeek && video.currentTime < liveEdge - 0.15) {
                    video.currentTime = target;
                }
            }

            // MSE removes every frame that depends on a removed frame up
            // to the next random access point. Cut only at a keyframe so
            // the remaining buffer stays decodable and P-frame appends are
            // not discarded.
            if (sourceBuffer.buffered.length && sourceBuffer.buffered.start(0) < video.currentTime - 2) {
                const target = video.currentTime - 1;
                let cut = null;
                for (const timestamp of keyframes) {
                    if (timestamp <= target) cut = timestamp;
                }
                if (cut !== null && cut > sourceBuffer.buffered.start(0)) {
                    await removeBuffered(sourceBuffer, 0, cut, abortSignal);
                    while (keyframes.length && keyframes[0] < cut) keyframes.shift();
                }
            }
        }
    } finally {
        await reader.cancel().catch(() => {});
        video.removeAttribute("src");
        video.load();
        URL.revokeObjectURL(objectUrl);
    }
}
