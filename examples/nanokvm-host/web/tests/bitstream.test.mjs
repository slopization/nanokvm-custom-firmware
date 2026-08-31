import assert from "node:assert/strict";
import test from "node:test";

import { annexBToSample, makeHvcc, parseHevcSps } from "../hevc.js";
import { ByteQueue, TimestampUnwrapper } from "../http-stream.js";
import { makeInitSegment, makeMediaFragment } from "../isobmff.js";

const fromHex = value => new Uint8Array(Buffer.from(value, "hex"));
const startCode = new Uint8Array([0, 0, 0, 1]);
const vps = fromHex("40010c01ffff0160000003000003000003000003005aac09");
const sps = fromHex("4201010160000003000003000003000003005aa003c08010e596b924c1ae5990");
const pps = fromHex("4401e072b02640");

test("builds the Wave420L HEVC decoder configuration", () => {
    const info = parseHevcSps(sps);
    assert.equal(info.width, 1920);
    assert.equal(info.height, 1080);
    assert.equal(info.bitDepthLumaMinus8, 0);
    const config = makeHvcc(vps, sps, pps);
    assert.equal(config.codec, "hvc1.1.6.L90");
    assert.equal(config.hvcc.length, 101);
});

test("converts Annex-B access units and builds ISO BMFF fragments", () => {
    const idr = fromHex("2601aabbcc");
    const accessUnit = new Uint8Array(
        startCode.length * 4 + vps.length + sps.length + pps.length + idr.length,
    );
    let offset = 0;
    for (const nal of [vps, sps, pps, idr]) {
        accessUnit.set(startCode, offset);
        offset += startCode.length;
        accessUnit.set(nal, offset);
        offset += nal.length;
    }
    const sample = annexBToSample(accessUnit);
    assert.deepEqual(sample.subarray(0, 4), new Uint8Array([0, 0, 0, idr.length]));
    const config = makeHvcc(vps, sps, pps);
    const init = makeInitSegment(1920, 1080, 90_000, config.hvcc);
    const fragment = makeMediaFragment(1, 0, 1500, true, sample);
    assert.equal(new TextDecoder().decode(init.subarray(4, 8)), "ftyp");
    assert.equal(new TextDecoder().decode(fragment.subarray(4, 8)), "moof");
    assert.ok(new TextDecoder().decode(fragment).includes("mdat"));
});

test("reads across transport chunks without retaining consumed bytes", async () => {
    const chunks = [new Uint8Array([1, 2]), new Uint8Array([3]), new Uint8Array([4, 5, 6])];
    const reader = {
        async read() {
            return chunks.length ? { value: chunks.shift(), done: false } : { done: true };
        },
    };
    const queue = new ByteQueue(reader);
    assert.deepEqual(await queue.take(4), new Uint8Array([1, 2, 3, 4]));
    assert.deepEqual(await queue.take(2), new Uint8Array([5, 6]));
});

test("unwraps timestamps and rejects non-wrap regressions", () => {
    const timestamps = new TimestampUnwrapper();
    assert.equal(timestamps.unwrap(0xffffff00), 0xffffff00);
    assert.equal(timestamps.unwrap(0xfffffff0), 0xfffffff0);
    assert.equal(timestamps.unwrap(0x20), 0x100000020);
    assert.throws(() => timestamps.unwrap(0x10), /moved backward/);
});
