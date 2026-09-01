"use strict";

import {tools, $} from "../tools.js";
import {wm} from "../wm.js";

export function Nanokvm() {
	let pairingTimer = null;

	const request = async function(path, options={}) {
		const response = await fetch(path, {
			...options,
			headers: {"Content-Type": "application/json", ...(options.headers || {})},
		});
		const body = await response.json().catch(() => ({message: response.statusText}));
		if (!response.ok) throw new Error(body.message || response.statusText);
		return body;
	};

	const number = id => Number($(id).value);
	const optionalNumber = id => ($(id).value === "" ? null : number(id));

	const loadSettings = async function() {
		const value = await request("/api/settings");
		$("nanokvm-name").value = value.name;
		$("nanokvm-codec").value = value.codec;
		$("nanokvm-audio").value = value.audio;
		$("nanokvm-usb-audio-class").value = value.usb_audio_class;
		$("nanokvm-encryption").value = value.encryption_mode;
		$("nanokvm-width").value = value.capture_width;
		$("nanokvm-height").value = value.capture_height;
		$("nanokvm-fps").value = value.advertised_fps;
		$("nanokvm-divider").value = value.frame_divider;
		$("nanokvm-bitrate").value = value.bitrate_kbps ?? "";
		$("nanokvm-gop").value = value.gop;
		$("nanokvm-fec").value = value.video_fec_percentage;
		$("nanokvm-parity").value = value.video_min_parity_shards;
		$("nanokvm-loop-filter").value = value.hevc_loop_filter ?? "";
		$("nanokvm-opus-rate").value = value.opus_bitrate_kbps;
		$("nanokvm-opus-complexity").value = value.opus_complexity;
		$("nanokvm-repeat-headers").checked = value.repeat_headers;
		$("nanokvm-aud").checked = value.aud;
	};

	const saveSettings = async function() {
		const button = $("nanokvm-save");
		tools.el.setEnabled(button, false);
		$("nanokvm-save-status").textContent = "Saving";
		try {
			const result = await request("/api/settings", {
				method: "PUT",
				body: JSON.stringify({
					name: $("nanokvm-name").value,
					codec: $("nanokvm-codec").value,
					audio: $("nanokvm-audio").value,
					usb_audio_class: $("nanokvm-usb-audio-class").value,
					encryption_mode: $("nanokvm-encryption").value,
					opus_bitrate_kbps: number("nanokvm-opus-rate"),
					opus_complexity: number("nanokvm-opus-complexity"),
					capture_width: number("nanokvm-width"),
					capture_height: number("nanokvm-height"),
					advertised_fps: number("nanokvm-fps"),
					frame_divider: number("nanokvm-divider"),
					bitrate_kbps: optionalNumber("nanokvm-bitrate"),
					gop: number("nanokvm-gop"),
					video_fec_percentage: number("nanokvm-fec"),
					video_min_parity_shards: number("nanokvm-parity"),
					hevc_loop_filter: optionalNumber("nanokvm-loop-filter"),
					repeat_headers: $("nanokvm-repeat-headers").checked,
					aud: $("nanokvm-aud").checked,
				}),
			});
			$("nanokvm-save-status").textContent = result.message;
			if (result.message.includes("USB reconnecting")) {
				setTimeout(() => tools.currentOpen("login"), 2000);
			}
		} catch (error) {
			$("nanokvm-save-status").textContent = error.message;
		} finally {
			tools.el.setEnabled(button, true);
		}
	};

	const deviceInfo = function(device) {
		const info = document.createElement("div");
		const name = document.createElement("div");
		name.className = "nanokvm-device-name";
		name.textContent = device.display_name || "Moonlight client";
		const id = document.createElement("div");
		id.className = "nanokvm-device-id";
		id.textContent = device.client_id;
		info.append(name, id);
		return info;
	};

	const actionButton = function(label, action) {
		const button = document.createElement("button");
		button.type = "button";
		button.textContent = label;
		button.onclick = action;
		return button;
	};

	const pendingDevice = function(device) {
		const row = document.createElement("div");
		row.className = "nanokvm-device";
		row.dataset.clientId = device.client_id;
		const actions = document.createElement("div");
		actions.className = "nanokvm-device-actions";
		const pin = document.createElement("input");
		pin.inputMode = "numeric";
		pin.maxLength = 4;
		pin.placeholder = "PIN";
		pin.setAttribute("aria-label", "Moonlight PIN");
		actions.append(
			pin,
			actionButton("Pair", async function() {
				try {
					await request(`/api/pairing/${encodeURIComponent(device.client_id)}`, {
						method: "POST",
						body: JSON.stringify({pin: pin.value}),
					});
					await loadDevices();
				} catch (error) {
					wm.error("Pairing failed", error.message);
				}
			}),
			actionButton("Reject", async function() {
				await request(`/api/pairing/${encodeURIComponent(device.client_id)}`, {method: "DELETE"});
				await loadDevices();
			}),
		);
		row.append(deviceInfo(device), actions);
		return row;
	};

	const pairedDevice = function(device) {
		const row = document.createElement("div");
		row.className = "nanokvm-device";
		row.dataset.clientId = device.client_id;
		row.append(deviceInfo(device), actionButton("Remove", async function() {
			if (!await wm.confirm("Remove this Moonlight device?")) return;
			await request(`/api/pairing/${encodeURIComponent(device.client_id)}`, {method: "DELETE"});
			await loadDevices();
		}));
		return row;
	};

	const renderDevices = function(target, devices, create) {
		target.replaceChildren();
		if (devices.length === 0) {
			const empty = document.createElement("div");
			empty.className = "nanokvm-empty";
			empty.textContent = "None";
			target.append(empty);
			return;
		}
		for (const device of devices) target.append(create(device));
	};

	const loadDevices = async function() {
		const devices = await request("/api/pairing");
		renderDevices($("nanokvm-pending-devices"), devices.pending, pendingDevice);
		renderDevices($("nanokvm-paired-devices"), devices.paired, pairedDevice);
	};

	tools.el.setOnClick($("nanokvm-save"), saveSettings);
	tools.el.setOnClick($("nanokvm-change-password"), async function() {
		try {
			await request("/api/password/change", {
				method: "POST",
				body: JSON.stringify({
					current_password: $("nanokvm-current-password").value,
					new_password: $("nanokvm-new-password").value,
				}),
			});
			tools.currentOpen("login");
		} catch (error) {
			wm.error("Password change failed", error.message);
		}
	});
	tools.el.setOnClick($("nanokvm-logout"), async function() {
		await request("/api/auth/logout", {method: "POST"});
		tools.currentOpen("login");
	});
	tools.el.setOnClick($("nanokvm-reboot"), async function() {
		if (!await wm.confirm("Reboot the KVM now?")) return;
		await request("/api/system/reboot", {method: "POST"});
		tools.el.setEnabled($("nanokvm-reboot"), false);
	});

	Promise.all([loadSettings(), loadDevices()]).catch(error => wm.error("NanoKVM settings failed", error.message));
	pairingTimer = setInterval(() => {
		if (!$("nanokvm-window").classList.contains("hidden")) loadDevices().catch(() => {});
	}, 2000);
	window.addEventListener("beforeunload", () => clearInterval(pairingTimer), {once: true});
}
