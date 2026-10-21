interface Point {
  x: number;
  y: number;
}

type Label = string;

function fail(point: Point, label: Label): never {
  throw new Error(`${label} at ${point.x},${point.y}`);
}

fail({ x: 1, y: 2 }, "boom");
