import { Route, Routes } from 'react-router';
import { LazyMotion, MotionConfig, domAnimation } from 'motion/react';
import { AppShell } from '@/components/shell/AppShell';
import { useAppStore } from '@/app/store';
import { useSystemReducedMotion } from '@/app/useSystemReducedMotion';
import { Toaster } from '@/components/ui/sonner';

// 动效用 Motion 的 LazyMotion + domAnimation（只用得到的那一组特性，不拉 domMax）。
// 「减少动效」有两半：html[data-reduced-motion] 管 CSS，MotionConfig 管 Motion，
// 两者都由同一个开关（跟随系统 / 开 / 关）决定。
export default function App() {
  const reducedPref = useAppStore((state) => state.reducedMotion);
  const systemReduced = useSystemReducedMotion();
  const reduced = reducedPref ?? systemReduced;

  // 路由名里带冒号（task:B2、settings:model），交给 AppShell 自己解析；
  // react-router 只负责 hash ↔ location 的同步。
  return (
    <LazyMotion features={domAnimation} strict>
      <MotionConfig reducedMotion={reduced ? 'always' : 'never'}>
        <Routes>
          <Route path="*" element={<AppShell />} />
        </Routes>
        {/* 第 1 期的 toast() 一直没挂载 Toaster（等于静默）；撤销提示条要靠它才看得见 */}
        <Toaster />
      </MotionConfig>
    </LazyMotion>
  );
}
