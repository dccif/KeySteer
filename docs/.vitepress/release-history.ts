import type { MarkdownRenderer } from 'vitepress'

/** Keep source headings intact for compose-release-notes.sh and stable anchors. */
export function releaseHistory(md: MarkdownRenderer): void {
  md.core.ruler.after('inline', 'release-history', state => {
    if (state.env.frontmatter?.releaseHistory !== true) return
    const output: typeof state.tokens = []
    let versions = 0
    let expanded = false
    let summary = false
    const html = (content: string) => {
      const token = new state.Token('html_block', '', 0)
      token.content = content + '\n'
      token.block = true
      output.push(token)
    }
    for (let index = 0; index < state.tokens.length; index++) {
      const token = state.tokens[index]
      if (token.type === 'heading_open' && token.tag === 'h2') {
        if (expanded) { html('</details>'); expanded = false }
        const title = state.tokens[index + 1]?.content ?? ''
        if (/^\d+\.\d+\.\d+(?:[-+].*)?$/.test(title) && versions++ > 0) {
          html('<details class="ks-release-history"><summary>')
          expanded = true
          summary = true
        }
      }
      output.push(token)
      if (summary && token.type === 'heading_close') {
        html('</summary>')
        summary = false
      }
    }
    if (expanded) html('</details>')
    state.tokens = output
  })
}
