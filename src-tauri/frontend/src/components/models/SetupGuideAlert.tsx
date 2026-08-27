import { CircleAlert, TriangleAlert } from "lucide-react";
import { Link } from "react-router-dom";
import { deriveSetupGuideIssues, type GuideIssue } from "@/components/models/setupGuide";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { useRuntime } from "@/providers/RuntimeContext";

/** 错误卡：单项直达对应配置页；多项每能力一个直达按钮。 */
function ErrorAlert({ issues }: { issues: GuideIssue[] }) {
  const single = issues.length === 1;
  return (
    <Alert variant="destructive">
      <CircleAlert className="h-4 w-4" />
      <AlertTitle>
        {single ? `${issues[0].name}出现错误` : `${issues.length} 项能力出现错误`}
      </AlertTitle>
      <AlertDescription>
        <div className="flex flex-wrap items-center gap-2">
          <span>
            {single
              ? "请到配置页查看详细原因并处理。"
              : `${issues.map((i) => i.name).join("、")}运行出错，请分别到配置页查看。`}
          </span>
          {issues.map((issue) => (
            <Button
              key={issue.capability}
              variant="outline"
              size="sm"
              className="shadow-none"
              asChild
            >
              <Link to={issue.href}>查看{issue.name}配置</Link>
            </Button>
          ))}
        </div>
      </AlertDescription>
    </Alert>
  );
}

/** 未配置卡：单项直达对应配置页；多项每能力一个直达按钮。 */
function UnconfiguredAlert({ issues }: { issues: GuideIssue[] }) {
  const single = issues.length === 1;
  return (
    <Alert variant="warning">
      <TriangleAlert className="h-4 w-4" />
      <AlertTitle>
        {single ? `${issues[0].name}尚未配置模型` : `${issues.length} 项能力尚未配置模型`}
      </AlertTitle>
      <AlertDescription>
        <div className="flex flex-wrap items-center gap-2">
          <span>
            {single
              ? "下载并配置模型后即可启用该能力。"
              : `${issues.map((i) => i.name).join("、")}需要模型支持，请分别到配置页下载。`}
          </span>
          {issues.map((issue) => (
            <Button
              key={issue.capability}
              variant="outline"
              size="sm"
              className="shadow-none"
              asChild
            >
              <Link to={issue.href}>去配置{issue.name}</Link>
            </Button>
          ))}
        </div>
      </AlertDescription>
    </Alert>
  );
}

/**
 * 模型与能力页顶部引导卡：存在「错误」或「未配置模型」的能力时给出下一步动作；
 * 全部正常时完全不渲染（不打扰日常使用）。
 */
export function SetupGuideAlert() {
  const runtime = useRuntime();
  const issues = deriveSetupGuideIssues(runtime);
  const errors = issues.filter((i) => i.kind === "error");
  const unconfigured = issues.filter((i) => i.kind === "unconfigured");
  if (issues.length === 0) return null;
  return (
    <>
      {errors.length > 0 && <ErrorAlert issues={errors} />}
      {unconfigured.length > 0 && <UnconfiguredAlert issues={unconfigured} />}
    </>
  );
}
