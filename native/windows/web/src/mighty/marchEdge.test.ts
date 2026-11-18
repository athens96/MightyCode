import { describe, expect, it } from 'vitest';
import { perimeter } from './camera';
import { marchEdge, type MarchPiece } from './marchEdge';

/** Whether the outline's own stroke is inked `at` along it, the dashes having marched `phase` of a period. */
const inked = (at: number, phase: number, dash: number[], period: number) => {
  const along = (((at - phase * period) % period) + period) % period;
  return along < dash[0]!;
};

/** Whether a piece inks its own line `along` units from its start. */
const pieceInked = (piece: MarchPiece, along: number, dash: number[], period: number) => {
  const at = (((along + piece.dashOffset) % period) + period) % period;
  return at < dash[0]!;
};

describe('the marching edge as pieces', () => {
  it('is four sides and four corners whose dashes are the outline stroke\'s, at every moment', () => {
    for (const [w, h] of [[360, 240], [600, 82], [283, 131], [240, 480]] as const) {
      const edge = marchEdge(w, h)!;
      expect(edge.pieces.map((p) => p.move)).toEqual(['x', 'x', 'y', 'y', 'turn', 'turn', 'turn', 'turn']);
      const r = 11, top = w - 2 - 2 * r, side = h - 2 - 2 * r, arc = (Math.PI * r) / 2;
      // The period divides the outline, so the dashes meet where the walk ends.
      expect(perimeter(w - 2, h - 2, r) / edge.period).toBeCloseTo(Math.round(perimeter(w - 2, h - 2, r) / edge.period), 6);
      const [topSide, bottomSide, rightSide, leftSide, topRight, bottomRight, bottomLeft, topLeft] = edge.pieces as [MarchPiece, MarchPiece, MarchPiece, MarchPiece, MarchPiece, MarchPiece, MarchPiece, MarchPiece];
      // Where each part starts along the outline, and how far along its own line the part's start is when nothing has moved.
      const parts: [MarchPiece, number, number][] = [
        [topSide, 0, edge.period], [topRight, top, (r * 3 * Math.PI) / 4], [rightSide, top + arc, edge.period], [bottomRight, top + arc + side, (r * 3 * Math.PI) / 4],
        [bottomSide, top + 2 * arc + side, edge.period], [bottomLeft, 2 * top + 2 * arc + side, (r * 3 * Math.PI) / 4],
        [leftSide, 2 * top + 3 * arc + side, edge.period], [topLeft, 2 * top + 3 * arc + 2 * side, (r * 3 * Math.PI) / 4],
      ];
      for (const [piece, start, lead] of parts) {
        for (const phase of [0, 0.13, 0.5, 0.77]) {
          for (const into of [0.25, 3.5, 9.75]) {
            // A move of `phase` brings the line's point `phase × period` earlier under the same place.
            const along = lead + into - phase * edge.period;
            expect(pieceInked(piece, along, edge.dash, edge.period), `${w}×${h} ${piece.move} at ${start}+${into} phase ${phase}`).toBe(inked(start + into, phase, edge.dash, edge.period));
          }
        }
      }
      // A side slides one period the way the outline runs; a corner turns by that period's angle.
      expect([topSide.by, bottomSide.by, rightSide.by, leftSide.by]).toEqual([edge.period, -edge.period, edge.period, -edge.period]);
      expect(topRight.by).toBeCloseTo(edge.period / r, 9);
      // Each side's line is one period longer than what shows of it.
      expect(topSide.box.width - topSide.clip.width).toBeCloseTo(edge.period, 6);
      expect(rightSide.box.height - rightSide.clip.height).toBeCloseTo(edge.period, 6);
      // The corners' quarters sit in the card's corners.
      expect([topRight.clip.left + topRight.clip.width, bottomRight.clip.top + bottomRight.clip.height, bottomLeft.clip.left, topLeft.clip.top]).toEqual([w, h, 0, 0]);
    }
  });
  it('leaves a card too small for quarter-circle corners to the stroked rectangle', () => {
    expect(marchEdge(20, 100)).toBeNull();
    expect(marchEdge(100, 20)).toBeNull();
    expect(marchEdge(Number.NaN, 100)).toBeNull();
    // A card just two radii wide has no top side, only corners and its upright sides.
    expect(marchEdge(24, 100)!.pieces.map((p) => p.move)).toEqual(['y', 'y', 'turn', 'turn', 'turn', 'turn']);
  });
});
