import { useCallback, useEffect, useRef, useState } from 'react';
import { open as openExternal } from '@tauri-apps/plugin-shell';
import {
  Smartphone,
  Copy,
  RefreshCw,
  PhoneOff,
  Ban,
  Settings2,
  Wallet,
  ExternalLink,
  Send,
  MessageSquareText,
  History,
  Loader2,
  KeyRound,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import { Badge, EmptyState, Modal, Spinner } from '../components/ui';
import { useAppStore } from '../store';
import { api } from '../lib/tauri';
import type { SmsCodeSettings } from '../types';
import { cn } from '../lib/cn';

const D1JIEMA_SIGNUP = 'https://www.d1jiema.com/appweb/signUp.html?inviter=1lngsgtr';
const D1JIEMA_PORTAL = 'https://www.d1jiema.com/appweb/signIn.html';

/** 收码轮询参数：3 秒一次，最长 120 秒 */
const POLL_INTERVAL_MS = 3000;
const POLL_MAX_MS = 120_000;

/** 从短信内容提取验证码（4~8 位数字） */
function extractCode(msg: string): string | null {
  const m = msg.match(/(?:验证码|校验码|code)[:：\s]*([0-9]{4,8})/i);
  if (m) return m[1];
  // 兜底：整条短信里唯一的 4~8 位连续数字
  const nums = msg.match(/[0-9]{4,8}/g);
  if (nums && nums.length === 1) return nums[0];
  return null;
}

// ======================== 设置弹窗 ========================

function SettingsModal({
  open,
  onClose,
  settings,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  settings: SmsCodeSettings;
  onSaved: () => void;
}) {
  const toast = useAppStore((s) => s.pushToast);
  const [token, setToken] = useState(settings.token);
  const [keyword, setKeyword] = useState(settings.keyword);
  const [cardType, setCardType] = useState(settings.card_type || '全部');
  const [province, setProvince] = useState(settings.province);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      setToken(settings.token);
      setKeyword(settings.keyword);
      setCardType(settings.card_type || '全部');
      setProvince(settings.province);
    }
  }, [open, settings]);

  const save = async () => {
    if (!token.trim()) {
      toast('warn', '请先填写 API Token（在 d1jiema 个人中心创建）');
      return;
    }
    if (!keyword.trim()) {
      toast('warn', '请填写短信关键词（如 Trae），否则可能收不到验证码');
      return;
    }
    setSaving(true);
    try {
      await api.smsCode.setSettings({
        token: token.trim(),
        keyword: keyword.trim(),
        card_type: cardType,
        province: province.trim(),
      });
      toast('success', '接码设置已保存');
      onSaved();
      onClose();
    } catch (err) {
      toast('error', `保存失败：${String(err)}`);
    } finally {
      setSaving(false);
    }
  };

  const input =
    'w-full rounded-lg border border-slate-200 bg-white px-3 py-2 text-sm outline-none transition focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100';

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="接码平台设置"
      footer={
        <>
          <button className="btn-ghost" onClick={onClose} disabled={saving}>
            取消
          </button>
          <button className="btn-primary" onClick={() => void save()} disabled={saving}>
            {saving ? <Spinner className="h-3.5 w-3.5" /> : null}
            保存
          </button>
        </>
      }
    >
      <div className="space-y-4">
        <div>
          <label className="mb-1 flex items-center gap-1.5 text-xs font-medium text-slate-600 dark:text-zinc-300">
            <KeyRound size={13} /> API Token（d1jiema 个人中心创建，永久有效）
          </label>
          <input
            className={input}
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="粘贴你的 API Token"
            autoComplete="off"
          />
          <p className="mt-1 text-xs text-slate-400">
            没有账号？
            <button
              className="ml-1 text-sky-600 hover:underline dark:text-sky-400"
              onClick={() => void openExternal(D1JIEMA_SIGNUP)}
            >
              注册 d1jiema
            </button>
            <span className="mx-1">·</span>
            已有账号？
            <button
              className="ml-1 text-sky-600 hover:underline dark:text-sky-400"
              onClick={() => void openExternal(D1JIEMA_PORTAL)}
            >
              登录创建 Token
            </button>
          </p>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="mb-1 block text-xs font-medium text-slate-600 dark:text-zinc-300">
              短信关键词
            </label>
            <input
              className={input}
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              placeholder="Trae"
            />
            <p className="mt-1 text-xs text-slate-400">只接收包含该关键词的短信</p>
          </div>
          <div>
            <label className="mb-1 block text-xs font-medium text-slate-600 dark:text-zinc-300">
              卡类型
            </label>
            <select
              className={input}
              value={cardType}
              onChange={(e) => setCardType(e.target.value)}
            >
              <option value="全部">全部</option>
              <option value="实卡">实体卡</option>
              <option value="虚卡">虚拟卡</option>
            </select>
          </div>
        </div>
        <div>
          <label className="mb-1 block text-xs font-medium text-slate-600 dark:text-zinc-300">
            归属地（留空 = 全部）
          </label>
          <input
            className={input}
            value={province}
            onChange={(e) => setProvince(e.target.value)}
            placeholder="如：广东（留空则不限归属地）"
          />
        </div>
      </div>
    </Modal>
  );
}

// ======================== 主页面 ========================

export default function RegisterAccount() {
  const toast = useAppStore((s) => s.pushToast);

  const [settings, setSettings] = useState<SmsCodeSettings | null>(null);
  const [hasToken, setHasToken] = useState(false);
  const [loading, setLoading] = useState(true);
  const [balance, setBalance] = useState<number | null>(null);
  const [balanceLoading, setBalanceLoading] = useState(false);

  const [phone, setPhone] = useState<string | null>(null);
  const [phoneBusy, setPhoneBusy] = useState(false);

  // 收短信轮询
  const [polling, setPolling] = useState(false);
  const [remainSec, setRemainSec] = useState(0);
  const [message, setMessage] = useState<string | null>(null);
  const pollTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  // 指定号码弹窗
  const [specOpen, setSpecOpen] = useState(false);
  const [specInput, setSpecInput] = useState('');

  // 释放/拉黑确认
  const [confirmRelease, setConfirmRelease] = useState(false);
  const [confirmBlock, setConfirmBlock] = useState(false);

  // 设置弹窗
  const [settingsOpen, setSettingsOpen] = useState(false);

  // 短信记录 Tab
  const [tab, setTab] = useState<'sms' | 'history'>('sms');
  const [history, setHistory] = useState<string[]>([]);
  const [historyAt, setHistoryAt] = useState<number | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);

  // 发送短信表单
  const [toPhone, setToPhone] = useState('');
  const [sendContent, setSendContent] = useState('');
  const [sending, setSending] = useState(false);

  const stopPoll = useCallback(() => {
    if (pollTimer.current) {
      clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
    setPolling(false);
    setRemainSec(0);
  }, []);

  const copy = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast('success', `${label}已复制`);
    } catch {
      toast('error', '复制失败');
    }
  };

  // 初始化：加载设置 + 余额
  const loadAll = useCallback(async () => {
    try {
      const s = await api.smsCode.getSettings();
      setSettings(s);
      setHasToken(s.token.length > 0);
      if (s.token.length > 0) {
        setBalanceLoading(true);
        api.smsCode
          .balance()
          .then(setBalance)
          .catch(() => {})
          .finally(() => setBalanceLoading(false));
      }
    } catch (err) {
      toast('error', `读取接码设置失败：${String(err)}`);
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void loadAll();
    return () => stopPoll(); // 离开页面停止轮询
  }, [loadAll, stopPoll]);

  // ---------------- 收短信轮询 ----------------

  const pollOnce = useCallback(
    async (p: string) => {
      try {
        const msg = await api.smsCode.getMsg(p);
        if (msg.includes('[尚未收到]')) return false;
        setMessage(msg);
        stopPoll();
        toast('success', '已收到短信');
        return true;
      } catch (err) {
        // getMsg 报错（如 token 失效）直接终止轮询
        toast('error', `查询短信失败：${String(err)}`);
        stopPoll();
        return true;
      }
    },
    [stopPoll, toast],
  );

  const startPoll = useCallback(
    (p: string) => {
      stopPoll();
      setMessage(null);
      setPolling(true);
      setRemainSec(Math.round(POLL_MAX_MS / 1000));
      void pollOnce(p);
      let elapsed = 0;
      pollTimer.current = setInterval(() => {
        elapsed += POLL_INTERVAL_MS;
        setRemainSec(Math.max(0, Math.round((POLL_MAX_MS - elapsed) / 1000)));
        if (elapsed >= POLL_MAX_MS) {
          stopPoll();
          toast('warn', '120 秒内未收到短信：建议释放号码后重新取号，反复收不到可拉黑');
          return;
        }
        void pollOnce(p);
      }, POLL_INTERVAL_MS);
    },
    [pollOnce, stopPoll, toast],
  );

  // ---------------- 手机号操作 ----------------

  const fetchPhone = async (specify?: string) => {
    setPhoneBusy(true);
    try {
      const p = await api.smsCode.getPhone(specify);
      setPhone(p);
      toast('success', `取号成功：${p}，正在等待短信…`);
      startPoll(p);
      // 取号可能扣费，刷新余额
      api.smsCode
        .balance()
        .then(setBalance)
        .catch(() => {});
    } catch (err) {
      toast('error', `取号失败：${String(err)}`);
    } finally {
      setPhoneBusy(false);
    }
  };

  const releasePhone = async () => {
    if (!phone) return;
    try {
      await api.smsCode.release(phone);
      toast('info', `号码 ${phone} 已释放`);
      stopPoll();
      setPhone(null);
      setMessage(null);
    } catch (err) {
      // 释放失败不重试（平台建议），号码仍从界面移除
      toast('warn', `释放返回：${String(err)}（已跳过，无需重试）`);
      stopPoll();
      setPhone(null);
    }
  };

  const blockPhone = async () => {
    if (!phone) return;
    try {
      await api.smsCode.block(phone);
      toast('info', `号码 ${phone} 已拉黑`);
      stopPoll();
      setPhone(null);
      setMessage(null);
    } catch (err) {
      toast('warn', `拉黑返回：${String(err)}（已跳过，无需重试）`);
      stopPoll();
      setPhone(null);
    }
  };

  // ---------------- 历史记录 ----------------

  const loadHistory = async () => {
    setHistoryLoading(true);
    try {
      const [rows, cached] = await api.smsCode.queryUsed();
      setHistory(rows);
      setHistoryAt(cached ? historyAt : Date.now());
      if (cached) toast('info', '平台限频 1 次/分钟：本次展示 60 秒内的缓存结果');
    } catch (err) {
      toast('error', `查询短信记录失败：${String(err)}`);
    } finally {
      setHistoryLoading(false);
    }
  };

  useEffect(() => {
    if (tab === 'history' && history.length === 0 && historyAt === null) {
      void loadHistory();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  // ---------------- 发送短信 ----------------

  const sendSms = async () => {
    if (!phone) {
      toast('warn', '请先获取手机号');
      return;
    }
    if (!toPhone.trim().startsWith('1069')) {
      toast('warn', '目标号码必须以 1069 开头（短信网关号）');
      return;
    }
    setSending(true);
    try {
      await api.smsCode.send(phone, toPhone.trim(), sendContent);
      toast('success', '短信已发送');
      setSendContent('');
    } catch (err) {
      toast('error', `发送失败：${String(err)}`);
    } finally {
      setSending(false);
    }
  };

  // ======================== 渲染 ========================

  if (loading) {
    return (
      <div className="flex h-64 items-center justify-center">
        <Spinner className="h-6 w-6 text-slate-400" />
      </div>
    );
  }

  // 未配置 Token：引导页
  if (!hasToken || !settings) {
    return (
      <div>
        <PageHeader title="注册账号" desc="对接 d1jiema 接码平台，取号收码辅助注册 Trae 账号" />
        <EmptyState
          icon={<Smartphone size={26} />}
          title="尚未配置接码平台 Token"
          hint="先注册 d1jiema 账号，在个人中心创建 API Token 后粘贴到设置中，即可开始取号收码"
        />
        <div className="mt-5 flex justify-center gap-2">
          <button className="btn-primary" onClick={() => void openExternal(D1JIEMA_SIGNUP)}>
            <ExternalLink size={14} /> 注册 d1jiema（推荐）
          </button>
          <button className="btn-ghost" onClick={() => void openExternal(D1JIEMA_PORTAL)}>
            <ExternalLink size={14} /> 登录创建 Token
          </button>
        </div>
        <div className="mt-3 text-center">
          <button
            className="text-sm text-sky-600 hover:underline dark:text-sky-400"
            onClick={() => setSettingsOpen(true)}
          >
            已有 Token？点此填写
          </button>
        </div>
        {settings && (
          <SettingsModal
            open={settingsOpen}
            onClose={() => setSettingsOpen(false)}
            settings={settings}
            onSaved={() => void loadAll()}
          />
        )}
      </div>
    );
  }

  const code = message ? extractCode(message) : null;

  return (
    <div>
      <PageHeader
        title="注册账号"
        desc="对接 d1jiema 接码平台：取号 → 等待验证码 → 注册后释放号码"
        actions={
          <>
            <Badge tone={balance === null ? 'slate' : balance > 1 ? 'green' : 'amber'}>
              <Wallet size={13} className="mr-1" />
              {balanceLoading ? '查询中…' : balance === null ? '余额未知' : `余额 ¥${balance.toFixed(2)}`}
            </Badge>
            <button
              className="btn-ghost !p-1.5"
              title="刷新余额"
              onClick={() => {
                setBalanceLoading(true);
                api.smsCode
                  .balance()
                  .then(setBalance)
                  .catch((e) => toast('error', `查询余额失败：${String(e)}`))
                  .finally(() => setBalanceLoading(false));
              }}
            >
              <RefreshCw size={15} />
            </button>
            <button className="btn-ghost" onClick={() => void openExternal(D1JIEMA_PORTAL)}>
              <ExternalLink size={14} /> 充值
            </button>
            <button className="btn-ghost" onClick={() => setSettingsOpen(true)}>
              <Settings2 size={14} /> 设置
            </button>
          </>
        }
      />

      {/* 手机号工作区 */}
      <div className="card p-4">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex min-w-56 items-center gap-2">
            <Smartphone size={18} className="text-slate-400" />
            {phone ? (
              <>
                <span className="font-mono text-lg font-semibold tabular-nums text-slate-800 dark:text-zinc-100">
                  {phone}
                </span>
                <button
                  className="btn-ghost !p-1.5"
                  title="复制手机号"
                  onClick={() => void copy(phone, '手机号')}
                >
                  <Copy size={14} />
                </button>
              </>
            ) : (
              <span className="text-sm text-slate-400">暂无号码</span>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button className="btn-primary" disabled={phoneBusy} onClick={() => void fetchPhone()}>
              {phoneBusy ? <Loader2 size={14} className="animate-spin" /> : <Smartphone size={14} />}
              获取手机号
            </button>
            <button className="btn-ghost" disabled={phoneBusy} onClick={() => setSpecOpen(true)}>
              指定号码
            </button>
            <button
              className="btn-ghost"
              disabled={!phone}
              onClick={() => setConfirmRelease(true)}
              title="释放后号码可被他人取用"
            >
              <PhoneOff size={14} /> 释放
            </button>
            <button
              className="btn-danger"
              disabled={!phone}
              onClick={() => setConfirmBlock(true)}
              title="收不到码的号码加入黑名单，避免再次取到"
            >
              <Ban size={14} /> 拉黑
            </button>
          </div>
          <div className="ml-auto flex items-center gap-2 text-xs text-slate-400">
            <span>关键词</span>
            <Badge tone="blue">{settings.keyword || '未设置'}</Badge>
            <span>卡类型</span>
            <Badge tone="violet">{settings.card_type || '全部'}</Badge>
            <span>归属地</span>
            <Badge tone="slate">{settings.province || '全部'}</Badge>
          </div>
        </div>
      </div>

      {/* 短信区 */}
      <div className="mt-4 flex gap-1">
        {(
          [
            { k: 'sms', label: '收短信 / 发送', icon: MessageSquareText },
            { k: 'history', label: '短信记录', icon: History },
          ] as const
        ).map(({ k, label, icon: Icon }) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            className={cn(
              'flex items-center gap-1.5 rounded-t-lg px-4 py-2 text-sm font-medium transition',
              tab === k
                ? 'bg-white text-slate-800 shadow-sm dark:bg-zinc-900 dark:text-zinc-100'
                : 'text-slate-500 hover:text-slate-700 dark:text-zinc-400',
            )}
          >
            <Icon size={15} /> {label}
          </button>
        ))}
      </div>

      {tab === 'sms' ? (
        <div className="card rounded-tl-none p-4">
          {/* 收短信状态 */}
          <div className="rounded-xl border border-slate-200 p-4 dark:border-zinc-700">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-sm font-medium text-slate-600 dark:text-zinc-300">收短信</span>
              {polling && (
                <span className="flex items-center gap-2 text-xs text-slate-400">
                  <Loader2 size={13} className="animate-spin" />
                  等待短信中… 剩余 {remainSec}s
                </span>
              )}
              {!polling && !message && phone && (
                <button
                  className="text-xs text-sky-600 hover:underline dark:text-sky-400"
                  onClick={() => startPoll(phone)}
                >
                  重新开始等待
                </button>
              )}
            </div>
            {!phone ? (
              <p className="text-xs text-slate-400">获取手机号后将自动开始等待验证码（每 3 秒查询一次，最长 120 秒）</p>
            ) : message ? (
              <div>
                <p className="whitespace-pre-wrap break-all font-mono text-sm text-slate-700 dark:text-zinc-200">
                  {message}
                </p>
                <div className="mt-3 flex flex-wrap gap-2">
                  {code && (
                    <button className="btn-primary" onClick={() => void copy(code, '验证码')}>
                      <Copy size={14} /> 复制验证码 {code}
                    </button>
                  )}
                  <button className="btn-ghost" onClick={() => void copy(message, '短信全文')}>
                    <Copy size={14} /> 复制全文
                  </button>
                </div>
              </div>
            ) : polling ? (
              <p className="text-xs text-slate-400">正在等待包含「{settings.keyword}」的短信…</p>
            ) : (
              <p className="text-xs text-slate-400">未在等待短信（120 秒超时或已停止）</p>
            )}
          </div>

          {/* 发送短信 */}
          <div className="mt-4 rounded-xl border border-slate-200 p-4 dark:border-zinc-700">
            <span className="mb-2 block text-sm font-medium text-slate-600 dark:text-zinc-300">
              发送短信（仅支持 1069 开头的网关号）
            </span>
            <div className="flex flex-wrap gap-2">
              <input
                className="w-44 rounded-lg border border-slate-200 bg-white px-3 py-2 text-sm outline-none focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
                value={toPhone}
                onChange={(e) => setToPhone(e.target.value)}
                placeholder="1069…"
              />
              <input
                className="min-w-52 flex-1 rounded-lg border border-slate-200 bg-white px-3 py-2 text-sm outline-none focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
                value={sendContent}
                onChange={(e) => setSendContent(e.target.value)}
                placeholder="短信内容"
              />
              <button className="btn-primary" disabled={sending || !phone} onClick={() => void sendSms()}>
                {sending ? <Loader2 size={14} className="animate-spin" /> : <Send size={14} />} 发送
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="card rounded-tl-none p-4">
          <div className="mb-3 flex items-center justify-between">
            <span className="text-sm font-medium text-slate-600 dark:text-zinc-300">
              最近 24 小时短信记录（平台限频：1 次/分钟）
            </span>
            <div className="flex items-center gap-2">
              {historyAt && (
                <span className="text-xs text-slate-400">
                  更新于 {new Date(historyAt).toLocaleTimeString()}
                </span>
              )}
              <button className="btn-ghost !p-1.5" title="刷新记录" onClick={() => void loadHistory()}>
                <RefreshCw size={14} className={historyLoading ? 'animate-spin' : ''} />
              </button>
            </div>
          </div>
          {history.length === 0 ? (
            <EmptyState icon={<History size={24} />} title="暂无短信记录" hint="取号收码后记录会显示在这里" />
          ) : (
            <div className="max-h-72 space-y-1 overflow-auto">
              {history.map((row, i) => (
                <div
                  key={i}
                  className="rounded-lg bg-slate-50 px-3 py-2 font-mono text-xs text-slate-600 dark:bg-zinc-800/60 dark:text-zinc-300"
                >
                  {row}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* 指定号码弹窗 */}
      <Modal
        open={specOpen}
        onClose={() => setSpecOpen(false)}
        title="指定手机号取号"
        footer={
          <>
            <button className="btn-ghost" onClick={() => setSpecOpen(false)}>
              取消
            </button>
            <button
              className="btn-primary"
              disabled={!/^1\d{10}$/.test(specInput.trim())}
              onClick={() => {
                setSpecOpen(false);
                void fetchPhone(specInput.trim());
                setSpecInput('');
              }}
            >
              取号
            </button>
          </>
        }
      >
        <p className="mb-2 text-xs text-slate-400">输入 11 位手机号，平台将尝试获取该指定号码</p>
        <input
          className="w-full rounded-lg border border-slate-200 bg-white px-3 py-2 text-sm outline-none focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
          value={specInput}
          onChange={(e) => setSpecInput(e.target.value.replace(/\D/g, '').slice(0, 11))}
          placeholder="13000000000"
        />
      </Modal>

      {/* 释放确认 */}
      <Modal
        open={confirmRelease}
        onClose={() => setConfirmRelease(false)}
        title="释放手机号"
        footer={
          <>
            <button className="btn-ghost" onClick={() => setConfirmRelease(false)}>
              取消
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                setConfirmRelease(false);
                void releasePhone();
              }}
            >
              确认释放
            </button>
          </>
        }
      >
        确定释放号码 <span className="font-mono">{phone}</span> 吗？释放后该号码可被其他用户取用。
      </Modal>

      {/* 拉黑确认 */}
      <Modal
        open={confirmBlock}
        onClose={() => setConfirmBlock(false)}
        title="拉黑手机号"
        footer={
          <>
            <button className="btn-ghost" onClick={() => setConfirmBlock(false)}>
              取消
            </button>
            <button
              className="btn-danger"
              onClick={() => {
                setConfirmBlock(false);
                void blockPhone();
              }}
            >
              确认拉黑
            </button>
          </>
        }
      >
        确定拉黑号码 <span className="font-mono">{phone}</span> 吗？拉黑后不会再取到该号码（收不到验证码的号码建议拉黑）。
      </Modal>

      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        settings={settings}
        onSaved={() => void loadAll()}
      />
    </div>
  );
}
