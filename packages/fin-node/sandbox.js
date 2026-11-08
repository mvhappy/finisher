// The sandbox status (plan 7C Task 3).
import { native, sync } from "./native.js";

export function sandboxStatus() {
  return JSON.parse(sync(() => native.sandboxStatus()));
}
