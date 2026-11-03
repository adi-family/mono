// Vite understands these; tsc needs telling. `*.css?inline` resolves to the compiled CSS string.
declare module "*.css";
declare module "*.css?inline" {
  const css: string;
  export default css;
}
