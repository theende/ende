function relative(file: string): string {
  return file.slice(Deno.cwd().length + 1);
}

export default {
  name: "test-plugin",
  rules: {
    "my-rule": {
      create(context) {
        return {
          VariableDeclarator(node) {
            console.log(`ctx.id:`);
            console.log(context.id);
            console.log();

            console.log(`ctx.filename:`);
            console.log(relative(context.filename));
            console.log();

            console.log(`ctx.getFilename():`);
            console.log(relative(context.getFilename()));
            console.log();

            console.log(`ctx.getSourceCode():`);
            console.log(context.getSourceCode() === context.sourceCode);
            console.log();
          },
        };
      },
    },
  },
} satisfies Deno.lint.Plugin;
