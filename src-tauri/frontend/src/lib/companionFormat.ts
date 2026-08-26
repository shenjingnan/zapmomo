/**
 * 伙伴 format 判别工具。
 *
 * 后端 `CompanionModel.format` 的取值空间："cubism3"（Live2D）/ "gif"（GIF 动图）/
 * "character"（角色包：character.md 人设 + character.png 静态立绘 + 可选 voice/ 音色）。
 */

/** 静态图像类伙伴（GIF / 角色包立绘）：用原生 <img> 渲染（GifStage），不走 PIXI。 */
export function isStaticImageFormat(format: string | null | undefined): boolean {
  return format === "gif" || format === "character";
}
