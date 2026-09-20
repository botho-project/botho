import { Buffer } from "buffer";
export declare function toBigIntLE(input: Buffer): bigint;
export declare function toBigIntBE(input: Buffer): bigint;
export declare function toBufferLE(value: bigint, width: number): Buffer;
export declare function toBufferBE(value: bigint, width: number): Buffer;
