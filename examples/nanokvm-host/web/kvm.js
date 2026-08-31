import { NativeHevcUnavailable, runNativeHevc } from "./mse-player.js";
import { runSoftwareHevc } from "./software-player.js";

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
    if (!start || !stop || !video || !canvas || !status) return;
    start.onclick = async () => {
        controller?.abort();
        const current = new AbortController();
        controller = current;
        start.disabled = true;
        stop.disabled = false;
        try {
            await runHevc(video, canvas, status, current.signal);
        } catch (error) {
            if (!current.signal.aborted) status.textContent = error.message;
        } finally {
            if (controller === current) {
                controller = null;
                start.disabled = false;
                stop.disabled = true;
            }
        }
    };
    stop.onclick = () => controller?.abort();
}

if (typeof document !== "undefined") init();
