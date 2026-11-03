import { Schema } from "effect";

/** Every error everywhere is `{ code, message, details, fix? }` (spec §19). */
export class FinError extends Schema.TaggedError<FinError>()("FinError", {
  code: Schema.String,
  message: Schema.String,
  details: Schema.Unknown,
  fix: Schema.optional(Schema.String),
}) {}
