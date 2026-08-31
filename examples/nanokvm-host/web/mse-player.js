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

async function appendPending(sourceBuffer, pending, duration, sequence, signal) {
    if (!Number.isSafeInteger(duration) || duration < 1 || duration > 0xffffffff) {
        throw new Error("invalid HEVC sample duration");
    }
    await append(
        sourceBuffer,
        makeMediaFragment(
            sequence,
            pending.timestamp,
            duration,
            pending.keyframe,
            annexBToSample(pending.bytes),
        ),
        signal,
    );
}

export async function runNativeHevc(video, status, abortSignal) {
    if (typeof MediaSource === "undefined") {
        throw new NativeHevcUnavailable("Media Source Extensions are unavailable");
    }
    const stream = await openHevcStream(abortSignal);
    const { reader, input, width, height, fps, timescale } = stream;
    const timestamps = new TimestampUnwrapper();
    let sourceBuffer = null;
    let sequence = 1;
    let firstTimestamp;
    let pending = null;
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
            if (pending) {
                await appendPending(
                    sourceBuffer,
                    pending,
                    current.timestamp - pending.timestamp,
                    sequence++,
                    abortSignal,
                );
                if (video.buffered.length) {
                    const liveEdge = video.buffered.end(video.buffered.length - 1);
                    if (video.currentTime < liveEdge - 0.15) video.currentTime = liveEdge - 0.05;
                }
                if (sourceBuffer.buffered.length && sourceBuffer.buffered.start(0) < video.currentTime - 2) {
                    await removeBuffered(sourceBuffer, 0, video.currentTime - 1, abortSignal);
                }
            }
            pending = current;
        }
    } finally {
        await reader.cancel().catch(() => {});
        video.removeAttribute("src");
        video.load();
        URL.revokeObjectURL(objectUrl);
    }
}
