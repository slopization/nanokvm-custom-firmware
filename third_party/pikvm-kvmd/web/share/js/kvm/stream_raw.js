"use strict";

import {tools, $} from "../tools.js";
import {NativeHevcUnavailable, runNativeHevc} from "/mse-player.js";
import {runSoftwareHevc} from "/software-player.js";

export function RawHevcStreamer(setActive, setInactive, setInfo, watchHook, organizeHook) {
	let controller = null;
	let retryTimer = null;
	let audio = null;
	let audioRetryTimer = null;
	let audioVolume = 0;
	let state = null;
	let resolution = {width: 1920, height: 1080};

	for (const id of ["stream-orient", "stream-mic", "stream-mic-raw", "stream-camera"]) {
		tools.feature.setEnabled($(id), false);
	}
	tools.feature.setEnabled($("stream-multimedia"), true);
	tools.feature.setEnabled($("stream-audio"), true);

	this.setOrientation = function() {};
	this.setAudioVolume = function(volume) {
		audioVolume = volume;
		if (audioVolume > 0 && state) {
			if (audio) audio.element.volume = audioVolume / 100;
			else startAudio();
		} else {
			stopAudio();
		}
	};
	this.setMicDevice = function() {};
	this.setMicRaw = function() {};
	this.setCameraDevice = function() {};
	this.getName = () => "HTTP HEVC";
	this.getMode = () => "raw-hevc";

	this.getResolution = function() {
		const canvas = $("stream-canvas");
		const video = $("stream-video");
		const element = canvas.classList.contains("hidden") ? video : canvas;
		return {
			"real_width": element.videoWidth || element.width || resolution.width,
			"real_height": element.videoHeight || element.height || resolution.height,
			"view_width": element.offsetWidth,
			"view_height": element.offsetHeight,
		};
	};

	this.ensureStream = function(nextState) {
		state = nextState;
		if (!state) {
			this.stopStream();
			return;
		}
		if (state.source?.resolution) resolution = state.source.resolution;
		if (!controller) start();
		if (audioVolume > 0 && !audio) startAudio();
	};

	this.stopStream = function() {
		state = null;
		clearTimeout(retryTimer);
		retryTimer = null;
		controller?.abort(new DOMException("stream stopped", "AbortError"));
		controller = null;
		stopAudio();
		setInactive();
		setInfo(false, false, "");
	};

	function statusTarget() {
		return {
			set textContent(text) {
				setInfo(true, state?.source?.online !== false, text);
				organizeHook();
			},
		};
	}

	function startAudio() {
		if (audio || audioVolume <= 0 || !state) return;
		clearTimeout(audioRetryTimer);
		audioRetryTimer = null;
		if (!window.MediaSource?.isTypeSupported('audio/webm; codecs="opus"')) {
			tools.error("HTTP Opus playback failed: Opus-in-WebM MSE is unavailable");
			return;
		}
		const stream = {
			controller: new AbortController(),
			element: new Audio(),
			mediaSource: new MediaSource(),
			objectUrl: null,
		};
		stream.element.preload = "auto";
		stream.element.volume = audioVolume / 100;
		stream.objectUrl = URL.createObjectURL(stream.mediaSource);
		stream.element.src = stream.objectUrl;
		audio = stream;
		stream.element.play().catch(function(error) {
			if (!stream.controller.signal.aborted) tools.error("HTTP Opus autoplay failed:", error);
		});
		runMseOpus(stream).catch(function(error) {
			if (!stream.controller.signal.aborted) {
				tools.error("HTTP Opus playback failed:", error);
				retryAudio(stream);
			}
		});
	}

	function stopAudio() {
		clearTimeout(audioRetryTimer);
		audioRetryTimer = null;
		if (audio) {
			const stream = audio;
			audio = null;
			stream.controller.abort(new DOMException("audio stopped", "AbortError"));
			stream.element.pause();
			stream.element.removeAttribute("src");
			stream.element.load();
			if (stream.objectUrl) URL.revokeObjectURL(stream.objectUrl);
		}
	}

	function retryAudio(stream) {
		if (audio !== stream) return;
		stopAudio();
		if (state && audioVolume > 0 && !audioRetryTimer) {
			audioRetryTimer = setTimeout(function() {
				audioRetryTimer = null;
				startAudio();
			}, 1000);
		}
	}

	async function runMseOpus(stream) {
		await once(stream.mediaSource, "sourceopen", stream.controller.signal);
		const source = stream.mediaSource.addSourceBuffer('audio/webm; codecs="opus"');
		const response = await fetch(`/api/kvm/audio?_=${Date.now()}`, {
			signal: stream.controller.signal,
			cache: "no-store",
		});
		if (!response.ok || !response.body) throw new Error(`audio HTTP ${response.status}`);
		const reader = new StreamReader(response.body.getReader());
		const preamble = await reader.readExact(16);
		if (new TextDecoder().decode(preamble.subarray(0, 8)) !== "NKOPUS01") {
			throw new Error("invalid Opus stream header");
		}
		const view = new DataView(preamble.buffer, preamble.byteOffset, preamble.byteLength);
		const sampleRate = view.getUint32(8);
		const channels = view.getUint16(12);
		const frameSamples = view.getUint16(14);
		if (sampleRate !== 48000 || channels !== 2 || frameSamples === 0) {
			throw new Error("unsupported Opus stream format");
		}
		await appendBuffer(source, webmOpusInit(sampleRate, channels, frameSamples), stream.controller.signal);

		let pending = [];
		while (!stream.controller.signal.aborted) {
			const header = await reader.readExact(8);
			const packetView = new DataView(header.buffer, header.byteOffset, header.byteLength);
			const size = packetView.getUint32(0);
			const timestamp = packetView.getUint32(4);
			if (size === 0 || size > 1275) throw new Error("invalid Opus packet size");
			pending.push(webmOpusCluster(Math.floor(timestamp * 1000 / sampleRate), await reader.readExact(size)));
			if (pending.length < 2) continue;
			await appendBuffer(source, concatBytes(...pending), stream.controller.signal);
			pending = [];
			await maintainLiveAudio(stream.element, source, stream.controller.signal);
			if (stream.element.paused) await stream.element.play();
		}
	}

	async function start() {
		controller = new AbortController();
		const ownController = controller;
		const video = $("stream-video");
		const canvas = $("stream-canvas");
		tools.hidden.setVisible($("stream-image"), false);
		tools.hidden.setVisible(canvas, false);
		tools.hidden.setVisible(video, true);
		setActive();
		setInfo(true, state?.source?.online !== false, "Connecting HTTP HEVC");
		watchHook();
		try {
			try {
				await runNativeHevc(video, statusTarget(), ownController.signal);
			} catch (error) {
				if (!(error instanceof NativeHevcUnavailable)) throw error;
				tools.info(`Native HEVC unavailable, using software decoder: ${error.message}`);
				await runSoftwareHevc(video, canvas, statusTarget(), ownController.signal);
			}
		} catch (error) {
			if (!ownController.signal.aborted) {
				tools.error("HTTP HEVC stream failed:", error);
				setInactive();
				setInfo(false, false, error.message || String(error));
			}
		} finally {
			if (controller === ownController) controller = null;
			if (state && !controller && !retryTimer) {
				retryTimer = setTimeout(function() {
					retryTimer = null;
					if (state && !controller) start();
				}, 1000);
			}
		}
	}
}

class StreamReader {
	constructor(reader) {
		this.reader = reader;
		this.pending = new Uint8Array(0);
	}

	async readExact(size) {
		while (this.pending.byteLength < size) {
			const {value, done} = await this.reader.read();
			if (done) throw new Error("audio stream ended");
			this.pending = concatBytes(this.pending, value);
		}
		const value = this.pending.slice(0, size);
		this.pending = this.pending.slice(size);
		return value;
	}
}

function once(target, event, signal) {
	if (event === "sourceopen" && target.readyState === "open") return Promise.resolve();
	return new Promise(function(resolve, reject) {
		const cleanup = function() {
			target.removeEventListener(event, ready);
			target.removeEventListener("error", failed);
			signal.removeEventListener("abort", aborted);
		};
		const ready = function() { cleanup(); resolve(); };
		const failed = function() { cleanup(); reject(new Error(`MSE ${event} failed`)); };
		const aborted = function() { cleanup(); reject(signal.reason); };
		target.addEventListener(event, ready, {once: true});
		target.addEventListener("error", failed, {once: true});
		signal.addEventListener("abort", aborted, {once: true});
	});
}

async function appendBuffer(source, bytes, signal) {
	if (signal.aborted) throw signal.reason;
	source.appendBuffer(bytes);
	await once(source, "updateend", signal);
}

async function maintainLiveAudio(element, source, signal) {
	if (source.buffered.length === 0) return;
	const start = source.buffered.start(0);
	const end = source.buffered.end(source.buffered.length - 1);
	if (!Number.isFinite(element.currentTime) || end - element.currentTime > 0.30) {
		element.currentTime = Math.max(start, end - 0.10);
	}
	if (end - start > 2.0) {
		source.remove(start, end - 1.0);
		await once(source, "updateend", signal);
	}
}

function webmOpusInit(sampleRate, channels, frameSamples) {
	const opusHead = concatBytes(
		textBytes("OpusHead"),
		new Uint8Array([1, channels, 0x38, 0x01]),
		littleEndian32(sampleRate),
		new Uint8Array([0, 0, 0]),
	);
	const audio = ebmlElement([0xe1], concatBytes(
		ebmlElement([0xb5], float64Bytes(sampleRate)),
		ebmlUint([0x9f], channels),
	));
	const track = ebmlElement([0xae], concatBytes(
		ebmlUint([0xd7], 1),
		ebmlUint([0x73, 0xc5], 1),
		ebmlUint([0x83], 2),
		ebmlUint([0x9c], 0),
		ebmlUint([0x23, 0xe3, 0x83], Math.round(frameSamples * 1e9 / sampleRate)),
		ebmlElement([0x86], textBytes("A_OPUS")),
		ebmlElement([0x63, 0xa2], opusHead),
		ebmlUint([0x56, 0xaa], 6500000),
		ebmlUint([0x56, 0xbb], 80000000),
		audio,
	));
	const info = ebmlElement([0x15, 0x49, 0xa9, 0x66], concatBytes(
		ebmlUint([0x2a, 0xd7, 0xb1], 1000000),
		ebmlElement([0x4d, 0x80], textBytes("NanoKVM")),
		ebmlElement([0x57, 0x41], textBytes("NanoKVM")),
	));
	const tracks = ebmlElement([0x16, 0x54, 0xae, 0x6b], track);
	const ebml = ebmlElement([0x1a, 0x45, 0xdf, 0xa3], concatBytes(
		ebmlUint([0x42, 0x86], 1),
		ebmlUint([0x42, 0xf7], 1),
		ebmlUint([0x42, 0xf2], 4),
		ebmlUint([0x42, 0xf3], 8),
		ebmlElement([0x42, 0x82], textBytes("webm")),
		ebmlUint([0x42, 0x87], 4),
		ebmlUint([0x42, 0x85], 2),
	));
	return concatBytes(
		ebml,
		new Uint8Array([0x18, 0x53, 0x80, 0x67, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
		info,
		tracks,
	);
}

function webmOpusCluster(timestampMs, packet) {
	const block = concatBytes(new Uint8Array([0x81, 0, 0, 0x80]), packet);
	return ebmlElement([0x1f, 0x43, 0xb6, 0x75], concatBytes(
		ebmlUint([0xe7], timestampMs),
		ebmlElement([0xa3], block),
	));
}

function ebmlElement(id, payload) {
	return concatBytes(new Uint8Array(id), ebmlSize(payload.byteLength), payload);
}

function ebmlUint(id, value) {
	let width = 1;
	while (value >= 2 ** (width * 8) && width < 8) ++width;
	const bytes = new Uint8Array(width);
	for (let index = width - 1; index >= 0; --index) {
		bytes[index] = value & 0xff;
		value = Math.floor(value / 256);
	}
	return ebmlElement(id, bytes);
}

function ebmlSize(size) {
	let width = 1;
	while (size >= 2 ** (width * 7) - 1 && width < 8) ++width;
	const bytes = new Uint8Array(width);
	for (let index = width - 1; index >= 0; --index) {
		bytes[index] = size & 0xff;
		size = Math.floor(size / 256);
	}
	bytes[0] |= 1 << (8 - width);
	return bytes;
}

function float64Bytes(value) {
	const bytes = new Uint8Array(8);
	new DataView(bytes.buffer).setFloat64(0, value);
	return bytes;
}

function littleEndian32(value) {
	const bytes = new Uint8Array(4);
	new DataView(bytes.buffer).setUint32(0, value, true);
	return bytes;
}

function textBytes(value) {
	return new TextEncoder().encode(value);
}

function concatBytes(...arrays) {
	const result = new Uint8Array(arrays.reduce((size, array) => size + array.byteLength, 0));
	let offset = 0;
	for (const array of arrays) {
		result.set(array, offset);
		offset += array.byteLength;
	}
	return result;
}
