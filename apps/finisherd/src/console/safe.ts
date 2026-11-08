// Text from a registry or a publisher is shown to the owner on the approval screen, so it must
// not be able to rewrite that screen (review M1 of Tasks 10 and 12): a terminal obeys escape
// sequences (clear, move the cursor, retitle the window, hide a line with `\r`), and a
// bidirectional override makes `evil.exe` read `exe.live`.

/** C0 and C1 controls (escape, carriage return, newline, bell, the 8-bit CSI …) and DEL. */
const CONTROLS = /[\u0000-\u001f\u007f-\u009f]/g;
/** Arabic letter mark, left-to-right/right-to-left marks, embeddings and overrides, isolates. */
const BIDI = /[؜‎‏‪-‮⁦-⁩]/g;

/**
 * `value` as one plain line: every control and bidirectional-formatting character removed.
 * Everything the console prints that a registry, a publisher or a package wrote goes through it
 * (names, titles, summaries, licences, permission details, ids, versions, paths).
 */
export const safe = (value: unknown): string => String(value).replace(CONTROLS, "").replace(BIDI, "");
