// The running edge over a card (MightyGraphActivity): dashes fitted to its outline, marching once per period.
import type { CSSProperties } from 'react';
import { dashForCard, dashOffset } from './camera';
import { marchEdge } from './marchEdge';

/**
 * The marching dashes over a `w` × `h` card: pieces the compositor moves (marchEdge.ts), so nothing is painted while they march.
 * A card too small for them gets the stroked rectangle, its dash offset stepped (mighty.css).
 */
export function MarchingEdge({ w, h, radius = 11 }: { w: number; h: number; radius?: number }) {
  if (w <= 2 || h <= 2) return null;
  const edge = marchEdge(w, h, radius);
  if (!edge) {
    const dash = dashForCard(w, h);
    const period = -dashOffset(1, dash);
    return (
      <svg className="mighty-outline outline-marching" width={w} height={h} aria-hidden="true" data-outline="marching">
        <rect x={1} y={1} width={w - 2} height={h - 2} rx={radius} ry={radius} style={{ strokeDasharray: dash.join(' '), '--march': `${-period}` } as CSSProperties} />
      </svg>
    );
  }
  const dashes = edge.dash.join(' ');
  return (
    <div className="mighty-march" style={{ width: w, height: h }} aria-hidden="true" data-outline="marching">
      {edge.pieces.map((piece, index) => (
        <div key={index} className="mighty-march-clip" style={piece.clip}>
          <div className={'mighty-march-move move-' + piece.move} style={{ ...piece.box, '--by': piece.move === 'turn' ? `${piece.by}rad` : `${piece.by}px` } as CSSProperties}>
            <svg width={piece.box.width} height={piece.box.height} aria-hidden="true" focusable="false">
              <path d={piece.d} strokeDasharray={dashes} strokeDashoffset={piece.dashOffset} />
            </svg>
          </div>
        </div>
      ))}
    </div>
  );
}
