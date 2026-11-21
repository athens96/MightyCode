/**
 * Escapes a value for safe interpolation inside a quoted CSS attribute selector (`[data-x="..."]`).
 * Uses `CSS.escape()` in browsers and falls back to escaping quotes, backslashes, brackets and control chars in Node.
 */
export function escapeCssAttr(value: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
    return CSS.escape(value);
  }
  let out = '';
  for (let i = 0; i < value.length; i += 1) {
    const ch = value[i]!;
    const code = value.charCodeAt(i);
    if (code === 0) {
      out += '\ufffd';
    } else if ((code >= 0x01 && code <= 0x1f) || code === 0x7f) {
      out += '\\' + code.toString(16) + ' ';
    } else if (ch === '"' || ch === '\\' || ch === '\'' || ch === '[' || ch === ']') {
      out += '\\' + ch;
    } else {
      out += ch;
    }
  }
  return out;
}
