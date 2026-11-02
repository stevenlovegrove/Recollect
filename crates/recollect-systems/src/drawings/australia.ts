// Australia as a map. A region takes its colour once it's pinned or there's only one left, and until
// then it shows the colours it could still be, which can be clicked to pin one.

const shapes: Record<string, { outline: string; label: [number, number] }> = {
  "Western Australia": { outline: "20,120 150,50 150,290 60,300 20,240", label: [85, 180] },
  "Northern Territory": { outline: "150,50 260,40 260,190 150,190", label: [205, 110] },
  "South Australia": { outline: "150,190 300,190 300,310 240,290 150,290", label: [225, 240] },
  Queensland: { outline: "260,40 300,20 340,110 420,190 300,190 260,190", label: [325, 145] },
  "New South Wales": { outline: "300,190 420,190 400,280 300,270", label: [355, 235] },
  Victoria: { outline: "300,270 400,280 372,340 300,325", label: [345, 293] },
  Tasmania: { outline: "305,352 395,352 378,412 322,412", label: [350, 372] },
};

const paints: Record<string, string> = { red: "#d9534f", green: "#4caf6d", blue: "#4a7fd6" };

export function draw(world: World): string {
  const regions = world.choices.map((c) => {
    const shape = shapes[c.name];
    if (!shape) return "";
    const left = c.options.filter((o) => o.possible);
    const settled = c.options.find((o) => o.picked) ?? (left.length === 1 ? left[0] : undefined);
    const [x, y] = shape.label;
    const fill = settled ? paints[settled.name] : "var(--panel)";
    const ink = settled ? "#ffffff" : "var(--text)";
    const pick = settled ? `data-pin-choice="${c.id}" data-pin-option="${settled.id}" style="cursor:pointer"` : "";
    const dots = settled
      ? ""
      : left
          .map((o, i) => {
            const dx = x + (i - (left.length - 1) / 2) * 22;
            return `<circle data-pin-choice="${c.id}" data-pin-option="${o.id}" cx="${dx}" cy="${y + 16}" r="8" fill="${paints[o.name]}" style="cursor:pointer" />`;
          })
          .join("");
    const none = left.length === 0 ? `<text x="${x}" y="${y + 20}" text-anchor="middle" font-size="11" fill="var(--muted)">no colour left</text>` : "";
    return `<polygon ${pick} points="${shape.outline}" fill="${fill}" stroke="var(--text)" stroke-width="1.5" />
      <text x="${x}" y="${y}" text-anchor="middle" font-size="11" fill="${ink}" style="pointer-events:none">${c.name}</text>${dots}${none}`;
  });
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 440 420" width="440">${regions.join("")}</svg>`;
}
