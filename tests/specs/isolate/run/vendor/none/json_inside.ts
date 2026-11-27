import data from "./data.json" with { type: "json" };
export function inside(): boolean {
  return data.inside;
}
