import { describe, expect, it } from "vitest";
import { PatchQueue } from "./settings-queue";

type S = { a: number; b: number; c: number };
const BASE: S = { a: 1, b: 1, c: 1 };

describe("PatchQueue", () => {
  it("shows pending patches over the confirmed value", () => {
    const q = new PatchQueue<S>();
    q.reset(BASE);
    q.begin({ a: 2 });
    expect(q.value()).toEqual({ a: 2, b: 1, c: 1 });
  });
  it("rolls a failed patch back", () => {
    const q = new PatchQueue<S>();
    q.reset(BASE);
    const id = q.begin({ a: 2 });
    q.fail(id);
    expect(q.value()).toEqual(BASE);
  });
  it("keeps a newer pending patch over an older call's response", () => {
    const q = new PatchQueue<S>();
    q.reset(BASE);
    const first = q.begin({ a: 2 });
    q.begin({ a: 3 });
    q.settle(first, { a: 2, b: 1, c: 1 });
    expect(q.value().a).toBe(3);
  });
  it("ignores an older response that arrives after a newer one", () => {
    const q = new PatchQueue<S>();
    q.reset(BASE);
    const first = q.begin({ a: 2 });
    const second = q.begin({ b: 5 });
    q.settle(second, { a: 2, b: 5, c: 1 });
    q.settle(first, { a: 2, b: 1, c: 1 });
    expect(q.value()).toEqual({ a: 2, b: 5, c: 1 });
  });
  it("keeps a local preview over responses until it is committed", () => {
    const q = new PatchQueue<S>();
    q.reset(BASE);
    const id = q.begin({ a: 2 });
    q.preview({ c: 9 });
    q.settle(id, { a: 2, b: 1, c: 1 });
    expect(q.value()).toEqual({ a: 2, b: 1, c: 9 });
    const commit = q.begin({ c: 9 });
    q.fail(commit);
    expect(q.value()).toEqual({ a: 2, b: 1, c: 1 });
  });
  it("has no value before the first reset", () => {
    expect(new PatchQueue<S>().current()).toBeNull();
  });
});
