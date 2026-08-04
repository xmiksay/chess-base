// Parse a numeric route param / query value (issue #212). Vue-router hands
// params and query entries over as `string | string[]`; anything non-numeric
// (or absent) maps to null so a mangled URL never poisons a fetch.
export function numericParam(v: unknown): number | null {
  if (Array.isArray(v)) v = v[0]
  if (typeof v !== 'string' || v.trim() === '') return null
  const n = Number(v)
  return Number.isInteger(n) && n >= 0 ? n : null
}
