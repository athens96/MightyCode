// The running edge's marching dashes as pieces the compositor moves (Marching.tsx): the outline's four straight sides each
// slide a dashed line by one dash period, and its four corners each turn a dashed circle by that period's angle. A dash offset
// animated on the outline itself (the SVG way) is laid out and painted again on every step, which keeps the page drawing for
// as long as a request runs; a transform on a layer is not. The pieces draw what the outline's own stroke draws: the same
// dashes, at the same place, at every moment of the period.
import { OutlineWidth, dashFitting, perimeter } from './camera';

export interface MarchBox { left: number; top: number; width: number; height: number }

export interface MarchPiece {
  /** What shows of the piece, in the card's border box. */
  clip: MarchBox;
  /** The box that moves, in the clip's coordinates. */
  box: MarchBox;
  /** The dashed line or circle, in the moving box's coordinates. */
  d: string;
  dashOffset: number;
  /** One period's move: along x or y by `by` px, or a turn of `by` radians about the box's centre. */
  move: 'x' | 'y' | 'turn';
  by: number;
}

export interface MarchEdge { dash: number[]; period: number; pieces: MarchPiece[] }

/** The most a corner may turn in a period: past it the circle's own seam would come into view. */
const MaximumTurn = 2;

const round = (value: number) => Math.round(value * 1000) / 1000;

/**
 * The pieces of a card's marching edge: the outline is the rounded rectangle one unit inside the `w` × `h` border box, as the
 * stroked `<rect x=1 y=1 width=w-2 height=h-2 rx=radius>` draws it, walked clockwise from the top side's start. Null where the
 * corners are not quarter circles (a card smaller than two radii) or the period turns a corner too far: the caller draws the
 * stroked rectangle then.
 */
export function marchEdge(w: number, h: number, radius = 11): MarchEdge | null {
  const width = w - OutlineWidth, height = h - OutlineWidth, r = radius;
  if (!(r > 0) || !(width >= 2 * r) || !(height >= 2 * r) || !Number.isFinite(width + height)) return null;
  const dash = dashFitting(perimeter(width, height, r));
  const period = dash[0]! + dash[1]!;
  if (!(period > 0) || period / r > MaximumTurn) return null;
  const half = OutlineWidth / 2;
  const top = width - 2 * r, side = height - 2 * r, arc = (Math.PI * r) / 2;
  const phase = (at: number) => round((((at % period) + period) % period));
  const pieces: MarchPiece[] = [];
  // Where each part starts along the outline.
  const starts = { top: 0, topRight: top, right: top + arc, bottomRight: top + arc + side, bottom: top + 2 * arc + side, bottomLeft: 2 * top + 2 * arc + side, left: 2 * top + 3 * arc + side, topLeft: 2 * top + 3 * arc + 2 * side };

  // A side: a line one period longer than the side, which slides that period the way the outline runs.
  if (top > 0) {
    pieces.push({ clip: { left: half + r, top: 0, width: top, height: OutlineWidth }, box: { left: -period, top: 0, width: top + period, height: OutlineWidth },
      d: `M0 ${half}H${round(top + period)}`, dashOffset: phase(starts.top), move: 'x', by: period });
    pieces.push({ clip: { left: half + r, top: h - OutlineWidth, width: top, height: OutlineWidth }, box: { left: 0, top: 0, width: top + period, height: OutlineWidth },
      d: `M${round(top + period)} ${half}H0`, dashOffset: phase(starts.bottom), move: 'x', by: -period });
  }
  if (side > 0) {
    pieces.push({ clip: { left: w - OutlineWidth, top: half + r, width: OutlineWidth, height: side }, box: { left: 0, top: -period, width: OutlineWidth, height: side + period },
      d: `M${half} 0V${round(side + period)}`, dashOffset: phase(starts.right), move: 'y', by: period });
    pieces.push({ clip: { left: 0, top: half + r, width: OutlineWidth, height: side }, box: { left: 0, top: 0, width: OutlineWidth, height: side + period },
      d: `M${half} ${round(side + period)}V0`, dashOffset: phase(starts.left), move: 'y', by: -period });
  }

  // A corner: a whole dashed circle about the corner's centre, of which the corner's quarter shows, turning by the period's
  // angle. The circle starts opposite the quarter's middle (225° past the quarter's start), so its seam never shows.
  const reach = r + half, size = 2 * reach, lead = (r * 3 * Math.PI) / 4;
  const corner = (clipLeft: number, clipTop: number, boxLeft: number, boxTop: number, startDegrees: number, start: number) => {
    const angle = ((startDegrees + 225) * Math.PI) / 180;
    const x = Math.cos(angle) * r, y = Math.sin(angle) * r;
    const d = `M${round(reach + x)} ${round(reach + y)}A${r} ${r} 0 0 1 ${round(reach - x)} ${round(reach - y)}A${r} ${r} 0 0 1 ${round(reach + x)} ${round(reach + y)}`;
    pieces.push({ clip: { left: clipLeft, top: clipTop, width: reach, height: reach }, box: { left: boxLeft, top: boxTop, width: size, height: size },
      d, dashOffset: phase(start - lead), move: 'turn', by: period / r });
  };
  corner(w - reach, 0, -reach, 0, -90, starts.topRight);
  corner(w - reach, h - reach, -reach, -reach, 0, starts.bottomRight);
  corner(0, h - reach, 0, -reach, 90, starts.bottomLeft);
  corner(0, 0, 0, 0, 180, starts.topLeft);
  return { dash: dash.map(round), period, pieces };
}
