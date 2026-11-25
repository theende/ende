import data from "./data.json" with { type: "json" };
export const manifest = { permissions: [] };
export function read(): unknown {
  return data;
}
