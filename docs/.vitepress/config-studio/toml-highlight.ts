/** Escape before emitting markup: pasted TOML is untrusted text. */
const escape = (text: string) => text.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]!)

/** Small display-only lexer, also accepts incomplete TOML while typing. */
export function highlightTomlCode(source: string): string {
  const tokens = /#[^\r\n]*|"""[\s\S]*?(?:"""|(?![\s\S]))|'''[\s\S]*?(?:'''|(?![\s\S]))|"(?:\\.|[^"\\\r\n])*"|'[^'\r\n]*'|^\s*\[\[?[^\r\n]*\]\]?|\b(?:true|false|inf|nan)\b|[+-]?\b\d[\w.:-]*|[A-Za-z_][\w-]*(?=\s*=)/gm
  let output = '', end = 0
  for (const match of source.matchAll(tokens)) {
    output += escape(source.slice(end, match.index))
    const token = match[0], trimmed = token.trimStart()
    const kind = trimmed.startsWith('#') ? 'comment' : trimmed.startsWith('[') ? 'section'
      : /^\s*=/.test(source.slice(match.index + token.length)) ? 'key'
      : /^["']/.test(token) ? 'string' : /^(true|false|inf|nan)$/.test(token) ? 'boolean'
      : /^[+\-\d]/.test(token) ? 'number' : 'key'
    output += `<span class="ks-syntax-${kind}">${escape(token)}</span>`
    end = match.index + token.length
  }
  return output + escape(source.slice(end))
}
