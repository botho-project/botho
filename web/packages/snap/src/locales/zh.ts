/**
 * Chinese (Simplified) message map (issue #1095). Typed against
 * `keyof typeof en`, so a missing/renamed key is a compile error.
 *
 * Wording reuses the web wallet's already-reviewed `zh` locale files
 * (`web-wallet/src/locales/zh/{wallet,pay,claim,contacts}.json`) for equivalent
 * strings — e.g. "金额" (amount), "网络手续费" (network fee), "收款人" (recipient),
 * "恢复助记词" (recovery phrase), "领取链接" (claim link) — so the Snap does not
 * fork a second, drifting Chinese translation.
 *
 * Chinese has no grammatical singular/plural, so `history.confirmationsOne` and
 * `history.confirmationsOther` are intentionally identical (measure word 个).
 */
import type { en } from './en';

export const zh: Record<keyof typeof en, string> = {
  'receive.heading': '接收 BTH',
  'receive.body':
    '分享此 Botho 隐私地址以接收资金。每笔付款都会在链上创建一个全新的一次性输出，' +
    '因此你的余额保持私密。',

  'balance.heading': 'Botho 余额',
  'balance.spendable': '可用',

  'common.node': '节点',

  'send.heading': '确认发送',
  'send.amount': '金额',
  'send.networkFee': '网络手续费',
  'send.total': '合计',
  'send.recipient': '收款人',

  'history.heading': '交易历史',
  'history.empty': '暂无交易。你收到的付款将显示在此处。',
  'history.spent': '已发送',
  'history.received': '已接收',
  'history.confirmationsOne': '{count} 个确认',
  'history.confirmationsOther': '{count} 个确认',
  'history.line': '区块 {height} · {confirmations} · {hash}',

  'contacts.heading': '联系人',
  'contacts.empty': '尚无已保存的联系人。添加一个即可重复使用某个 Botho 地址，无需重新粘贴。',

  'claim.previewHeading': '领取链接',
  'claim.confirmHeading': '确认领取',
  'claim.body': '此领取链接包含将被扫入你钱包的资金。扫取手续费将从链接中支付。',
  'claim.empty': '没有可领取的内容——此链接为空、已被领取或尚未确认。',
  'claim.claimable': '可领取',
  'claim.sweepFee': '扫取手续费',
  'claim.youReceive': '你将收到',
  'claim.hint': '链接提示：{amount}（仅供参考——以上方扫描到的金额为准）',

  'request.heading': '付款请求',
  'request.body': '有人正在请求付款。请核对下方的金额和收款人，然后确认以从你的钱包付款。',
  'request.amount': '请求金额',
  'request.amountAny': '任意金额（由你决定）',
  'request.memo': '备注',
  'request.payTo': '付款至',

  'mnemonic.heading': 'Botho 恢复助记词',
  'mnemonic.body':
    '这 24 个单词由你的 MetaMask 秘密恢复助记词派生而来，拥有此 Botho 钱包的全部支配权。' +
    '请将它们记下并离线保管。任何看到它们的人都可以花费你的资金。',
  'mnemonic.placeholder': '•••• •••• （确认后显示） ••••',

  'error.rejectMnemonic': '用户拒绝显示恢复助记词。',
  'error.rejectSend': '用户拒绝了发送。',
  'error.rejectClaim': '用户拒绝了领取。',
};
