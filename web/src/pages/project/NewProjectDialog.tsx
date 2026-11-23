/**
 * 新建项目对话框。名字/一句目标/可选默认目录；重名由服务端拒绝，
 * 建完直接打开项目页；侧栏的「+」和空态入口共用。
 */

import { useEffect, useState } from 'react';
import { go } from '@/app/navigate';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { ServiceHttpError } from '@/data/service';
import { useProjectStore } from './projectStore';

export function NewProjectDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const createProject = useProjectStore((state) => state.create);
  const [name, setName] = useState('');
  const [goal, setGoal] = useState('');
  const [dir, setDir] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setName('');
    setGoal('');
    setDir('');
    setError(null);
  }, [open]);

  async function submit() {
    if (!name.trim() || saving) return;
    setSaving(true);
    setError(null);
    try {
      const project = await createProject({
        name: name.trim(),
        goal: goal.trim(),
        dir: dir.trim() || null,
      });
      onOpenChange(false);
      go('project:' + project.id);
    } catch (reason) {
      setError(
        reason instanceof ServiceHttpError && reason.detail
          ? reason.detail
          : reason instanceof Error
            ? reason.message
            : String(reason),
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[460px]" data-kind="new-project-dialog">
        <DialogHeader>
          <DialogTitle>新建项目</DialogTitle>
          <DialogDescription>名字必有；目标和默认目录可以以后改。</DialogDescription>
        </DialogHeader>
        <div className="space-y-3 py-1">
          <label className="block">
            <span className="mb-1 block text-1 text-ink-3">名字</span>
            <input
              autoFocus
              data-kind="new-project-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="比如：清河县农技站"
              className="h-8 w-full rounded-btn border border-line-2 bg-pane px-2 text-2 text-ink outline-none focus:border-brand"
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-1 text-ink-3">一句目标</span>
            <input
              data-kind="new-project-goal"
              value={goal}
              onChange={(event) => setGoal(event.target.value)}
              placeholder="这个项目要变成什么"
              className="h-8 w-full rounded-btn border border-line-2 bg-pane px-2 text-2 text-ink outline-none focus:border-brand"
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-1 text-ink-3">默认目录（可不填）</span>
            <input
              data-kind="new-project-dir"
              value={dir}
              onChange={(event) => setDir(event.target.value)}
              placeholder="~/projects/..."
              className="h-8 w-full rounded-btn border border-line-2 bg-pane px-2 text-2 text-ink outline-none focus:border-brand"
            />
          </label>
          {error ? (
            <p data-kind="new-project-error" className="text-1 text-fail">
              {error}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="rounded-btn border border-line-2 px-3 py-1.5 text-2 text-ink-2 transition-colors hover:bg-hover"
          >
            取消
          </button>
          <button
            type="button"
            data-kind="new-project-submit"
            disabled={!name.trim() || saving}
            onClick={() => void submit()}
            className="rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving ? '建…' : '创建'}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
