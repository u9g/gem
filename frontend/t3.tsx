import type { LucideIcon } from 'lucide-react';
import {
  Bot,
  ChevronRight,
  CircleAlert,
  GitPullRequestCreate,
  Hammer,
  MessageCircle,
  RotateCcw,
  SquarePen,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, useWindowDimensions, View } from 'react-native';

import type { Node } from './db';
import type { Proposal } from './shapes';

// T3 Code's dark-theme roles, over this app's background and text colors.
export const t3 = {
  background: '#19212A',
  foreground: '#D0CFD4',
  muted: 'rgba(208, 207, 212, 0.55)',
  faint: 'rgba(208, 207, 212, 0.35)',
  border: 'rgba(255, 255, 255, 0.06)',
  hover: 'rgba(255, 255, 255, 0.04)',
  input: 'rgba(255, 255, 255, 0.08)',
  primary: '#3767E8',
  addition: '#10B981',
  deletion: '#F26D6D',
  warning: '#F59E0B',
  mono: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
};

export type DiffFile = { path: string; added: number; removed: number };

export function diffFiles(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  for (const line of diff.split('\n')) {
    if (line.startsWith('+++ ')) files.push({ path: line.slice(6), added: 0, removed: 0 });
    else if (line.startsWith('+') && !line.startsWith('+++')) files[files.length - 1].added++;
    else if (line.startsWith('-') && !line.startsWith('---') && files.length) files[files.length - 1].removed++;
  }
  return files;
}

// T3 Code's work-log grouping: one sentence for what approving does.
export function stepSentence(proposal: Node<Proposal>): string {
  const patch = proposal.node('patch');
  const others = proposal.list('otherPatches').length;
  const phrases: string[] = [];
  if (proposal.str('followUp')) phrases.push('start a follow-up agent');
  if (patch) {
    const n = diffFiles(patch.str('diff')).length;
    phrases.push(`change ${n} ${n === 1 ? 'file' : 'files'}`);
  }
  if (others) {
    const n = others;
    phrases.push(`change ${n} other ${n === 1 ? 'pull request' : 'pull requests'}`);
  }
  if (proposal.has('openPullRequest')) phrases.push('open a pull request');
  if (proposal.str('reply')) phrases.push(proposal.str('threadId') ? 'reply in thread' : 'quote reply');
  if (proposal.bool('resolveThread')) phrases.push('resolve');
  if (proposal.bool('rerun')) phrases.push('rerun failed jobs');
  if (proposal.bool('closePullRequest')) phrases.push('close the pull request');
  if (proposal.str('requestedTool')) phrases.push('needs a new tool');
  const sentence =
    phrases.length > 1 ? `${phrases.slice(0, -1).join(', ')} and ${phrases[phrases.length - 1]}` : phrases[0] ?? '';
  return sentence.charAt(0).toUpperCase() + sentence.slice(1);
}

export function DiffStat({ added, removed }: { added: number; removed: number }) {
  return (
    <Text style={styles.stat}>
      <Text style={{ color: t3.addition }}>+{added}</Text> <Text style={{ color: t3.deletion }}>−{removed}</Text>
    </Text>
  );
}

export function Icon({ icon: I, color = t3.muted, size = 16 }: { icon: LucideIcon; color?: string; size?: number }) {
  return (
    <View style={styles.iconSlot}>
      <I size={size} color={color} strokeWidth={2} />
    </View>
  );
}

// One 24px line: icon, a truncated label, trailing content, and a chevron when it expands.
export function Row({
  icon,
  label,
  labelColor = t3.muted,
  trailing,
  children,
  initiallyOpen = false,
  indent = 0,
}: {
  icon?: LucideIcon;
  label: ReactNode;
  labelColor?: string;
  trailing?: ReactNode;
  children?: ReactNode;
  initiallyOpen?: boolean;
  indent?: number;
}) {
  const [open, setOpen] = useState(initiallyOpen);
  const [hovered, setHovered] = useState(false);
  return (
    <View style={{ marginLeft: indent }}>
      <Pressable
        onPress={children ? () => setOpen(!open) : undefined}
        onHoverIn={() => setHovered(true)}
        onHoverOut={() => setHovered(false)}
        style={[styles.row, hovered && children !== undefined && { backgroundColor: t3.hover }]}
      >
        {icon && <Icon icon={icon} />}
        <Text numberOfLines={1} style={[styles.label, { color: labelColor }]}>
          {label}
        </Text>
        {trailing}
        {children !== undefined && (
          <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
            <ChevronRight size={12} color={t3.muted} />
          </View>
        )}
      </Pressable>
      {open && <View style={styles.detail}>{children}</View>}
    </View>
  );
}

// Narrower than this, the app is laid out for a phone: one screen at a time and stacked cards.
export function usePhone(): boolean {
  return useWindowDimensions().width < 700;
}

export function Button({
  label,
  variant = 'outline',
  disabled = false,
  large = false,
  onPress,
}: {
  label: string;
  variant?: 'default' | 'outline' | 'ghost';
  disabled?: boolean;
  // Sized for a thumb, sharing its row's width with the other large buttons.
  large?: boolean;
  onPress?: () => void;
}) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      style={[
        styles.button,
        large && styles.buttonLarge,
        disabled && { opacity: 0.5 },
        variant === 'default' && { backgroundColor: t3.primary, borderColor: t3.primary },
        variant === 'outline' && { backgroundColor: t3.input, borderColor: t3.input },
        variant === 'ghost' && { borderColor: 'transparent' },
      ]}
    >
      <Text style={[styles.buttonText, large && styles.buttonTextLarge, variant === 'ghost' && { color: t3.muted }]}>
        {label}
      </Text>
    </Pressable>
  );
}

export function Code({ children }: { children: string }) {
  return (
    <View style={styles.codeBox}>
      <Text style={styles.code}>{children}</Text>
    </View>
  );
}

export function Markdown({ children }: { children: string }) {
  return <Text style={styles.markdown}>{children.trim()}</Text>;
}

export function Sidebar({ title, children }: { title: string; children: ReactNode }) {
  return (
    <View style={styles.sidebar}>
      <View style={styles.sidebarHeader}>
        <Text style={styles.sidebarTitle}>{title}</Text>
      </View>
      {children}
    </View>
  );
}

export function proposalIcon(proposal: Node<Proposal>) {
  if (proposal.str('error')) return CircleAlert;
  if (proposal.str('requestedTool')) return Hammer;
  if (proposal.has('patch') || proposal.list('otherPatches').length) return SquarePen;
  if (proposal.has('openPullRequest')) return GitPullRequestCreate;
  if (proposal.str('followUp')) return Bot;
  if (proposal.bool('rerun')) return RotateCcw;
  return MessageCircle;
}

const styles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    minHeight: 24,
    paddingHorizontal: 2,
    borderRadius: 8,
  },
  iconSlot: {
    width: 24,
    height: 24,
    alignItems: 'center',
    justifyContent: 'center',
    opacity: 0.7,
  },
  label: {
    flex: 1,
    fontSize: 14,
    lineHeight: 22,
  },
  detail: {
    marginLeft: 30,
    marginTop: 2,
    marginBottom: 6,
    gap: 6,
  },
  stat: {
    fontFamily: t3.mono,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  button: {
    height: 24,
    paddingHorizontal: 7,
    borderRadius: 8,
    borderWidth: 1,
    justifyContent: 'center',
  },
  buttonLarge: {
    flex: 1,
    height: 44,
    borderRadius: 12,
    alignItems: 'center',
  },
  buttonText: {
    color: t3.foreground,
    fontSize: 12,
    fontWeight: '500',
  },
  buttonTextLarge: {
    fontSize: 15,
  },
  codeBox: {
    maxHeight: 240,
    overflow: 'scroll',
    padding: 8,
    borderRadius: 8,
    backgroundColor: t3.hover,
  },
  code: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 18,
  },
  markdown: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
  },
  sidebar: {
    width: '100%',
    maxWidth: 420,
    minHeight: 900,
    backgroundColor: t3.background,
    borderLeftWidth: 1,
    borderLeftColor: t3.border,
  },
  sidebarHeader: {
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  sidebarTitle: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
  },
});
