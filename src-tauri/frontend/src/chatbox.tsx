import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ChatboxBar } from "@/components/chatbox/ChatboxBar";
import "./index.css";

const rootElement = document.getElementById("root");
if (!rootElement) throw new Error("找不到根元素 #root");

createRoot(rootElement).render(
  <StrictMode>
    <ChatboxBar />
  </StrictMode>,
);
