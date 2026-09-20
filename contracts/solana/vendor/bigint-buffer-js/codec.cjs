"use strict";

// Both entry points supply their Buffer implementation explicitly. The browser
// path does not depend on a bundler injecting a global Buffer or a native addon.
module.exports = function createCodec(Buffer) {
  // Audited SPL callers use 8, 16, 24 and 32 bytes. Allow bounded general codecs
  // while preventing untrusted widths/input lengths from causing huge work.
  const MAX_BYTES = 1024;

  function decode(input, littleEndian) {
    if (!Buffer.isBuffer(input)) throw new TypeError("expected a Buffer");
    if (input.length > MAX_BYTES)
      throw new RangeError("buffer exceeds 1024 bytes");
    let value = 0n;
    for (let i = 0; i < input.length; i++) {
      const offset = littleEndian ? input.length - 1 - i : i;
      value = (value << 8n) | BigInt(input[offset]);
    }
    return value;
  }

  function encode(value, width, littleEndian) {
    if (typeof value !== "bigint")
      throw new TypeError("expected an unsigned bigint");
    if (!Number.isSafeInteger(width) || width < 0 || width > MAX_BYTES) {
      throw new RangeError("width must be an integer from 0 to 1024");
    }
    // Check fit before allocating. For width zero only value zero is valid.
    if (value < 0n || value >> BigInt(width * 8) !== 0n) {
      throw new RangeError("unsigned value does not fit width");
    }
    const output = Buffer.alloc(width);
    for (let i = 0; i < width; i++) {
      output[littleEndian ? i : width - 1 - i] = Number(value & 255n);
      value >>= 8n;
    }
    return output;
  }

  return {
    toBigIntLE: (input) => decode(input, true),
    toBigIntBE: (input) => decode(input, false),
    toBufferLE: (value, width) => encode(value, width, true),
    toBufferBE: (value, width) => encode(value, width, false),
  };
};
