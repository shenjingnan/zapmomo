import { ArrowLeft } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";
import { SpeakerEnableCard } from "@/components/speaker/SpeakerEnableCard";
import { SpeakerEnrollDialog } from "@/components/speaker/SpeakerEnrollDialog";
import { SpeakerModelCard } from "@/components/speaker/SpeakerModelCard";
import { SpeakerParamsCard } from "@/components/speaker/SpeakerParamsCard";
import { SpeakerRosterCard } from "@/components/speaker/SpeakerRosterCard";
import { SpeakerTestDialog } from "@/components/speaker/SpeakerTestDialog";

/**
 * 声纹识别（Speaker Recognition）配置页：
 * 启用开关 + 说话人管理（录音注册/删除/识别测试）+ 声纹模型下载 + 识别参数。
 */
export function SpeakerPage() {
  const [enrollOpen, setEnrollOpen] = useState(false);
  const [testOpen, setTestOpen] = useState(false);

  return (
    <div className="space-y-4">
      <Link
        to="/models"
        className="inline-flex items-center gap-1.5 text-sm text-text-secondary transition-colors hover:text-text-primary"
      >
        <ArrowLeft className="h-4 w-4" />
        模型与能力
      </Link>

      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <h1 className="text-2xl font-semibold tracking-tight text-text-primary">
          声纹识别（Speaker Recognition）
        </h1>
      </header>

      <SpeakerEnableCard />

      <SpeakerRosterCard
        onEnrollOpen={() => setEnrollOpen(true)}
        onTestOpen={() => setTestOpen(true)}
      />

      <SpeakerModelCard />

      <SpeakerParamsCard />

      <SpeakerEnrollDialog open={enrollOpen} onClose={() => setEnrollOpen(false)} />

      <SpeakerTestDialog open={testOpen} onClose={() => setTestOpen(false)} />
    </div>
  );
}
