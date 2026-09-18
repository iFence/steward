const js = require("@eslint/js");
const tseslint = require("typescript-eslint");

module.exports = tseslint.config(
  { ignores: ["**/dist/**", "**/node_modules/**"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      // A leading underscore marks a binding that exists to satisfy a calling
      // convention rather than to be read — most visibly `command(name, input)`,
      // whose signature the plugin runtime fixes regardless of what a plugin
      // needs. `typescript-eslint` recommends exactly this pattern, and this
      // repo already names such parameters `_`-prefixed.
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
    },
  },
);
