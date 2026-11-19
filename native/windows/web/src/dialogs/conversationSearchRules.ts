// 대화 검색's page rules (ConversationSearch.tsx): the query's terms as the host splits them, the snippet's matched words to mark,
// and the arrow keys' move through the hits. Pure.

/** Lower-cased terms of a query, split on white space (mighty_history::conversation_search::terms). */
export function searchTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter((term) => term.length > 0);
}

export interface SnippetPart {
  text: string;
  match: boolean;
}

/** The snippet in parts, every place one of the query's terms stands in it (ignoring case) marked; overlapping places are joined. */
export function highlight(snippet: string, query: string): SnippetPart[] {
  const terms = searchTerms(query);
  if (terms.length === 0 || snippet.length === 0) return snippet ? [{ text: snippet, match: false }] : [];
  // The lowered text, each of its UTF-16 units knowing the unit of `snippet` it came from (lowering may change the length).
  let lower = '';
  const origin: number[] = [];
  for (let at = 0; at < snippet.length;) {
    const point = snippet.codePointAt(at)!;
    const character = String.fromCodePoint(point);
    const lowered = character.toLowerCase();
    for (let unit = 0; unit < lowered.length; unit++) origin.push(at);
    lower += lowered;
    at += character.length;
  }
  origin.push(snippet.length);
  const marked = new Array<boolean>(snippet.length).fill(false);
  for (const term of terms) {
    for (let from = lower.indexOf(term); from >= 0; from = lower.indexOf(term, from + 1)) {
      const start = origin[from]!;
      const last = origin[from + term.length - 1]!;
      const end = last + (snippet.codePointAt(last)! > 0xffff ? 2 : 1);
      for (let index = start; index < end; index++) marked[index] = true;
    }
  }
  const parts: SnippetPart[] = [];
  for (let index = 0; index < snippet.length; index++) {
    const match = marked[index]!;
    const previous = parts[parts.length - 1];
    if (previous && previous.match === match) previous.text += snippet[index];
    else parts.push({ text: snippet[index]!, match });
  }
  return parts;
}

/** The hit chosen after a key in the search field: ↑/↓ one step, PageUp/PageDown five (held at the ends); null for any other key. */
export function moveSelection(current: number, key: string, count: number): number | null {
  if (count === 0) return null;
  switch (key) {
    case 'ArrowDown': return Math.min(current + 1, count - 1);
    case 'ArrowUp': return Math.max(current - 1, 0);
    case 'PageDown': return Math.min(current + 5, count - 1);
    case 'PageUp': return Math.max(current - 5, 0);
    default: return null;
  }
}
