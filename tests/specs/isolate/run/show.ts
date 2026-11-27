// Prints what a call returns, or the error it throws.
export async function show(label: string, call: () => unknown) {
  try {
    console.log(`${label}: ${JSON.stringify(await call())}`);
  } catch (e) {
    console.log(`${label}: ${(e as Error).name}: ${(e as Error).message}`);
  }
}
