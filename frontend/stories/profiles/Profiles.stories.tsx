import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Check, ChevronDown, ChevronsUpDown, Plus } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ClusterRow, PhoneClusterRow } from '../../ClusterRow';
import { type PullRequestCluster, PullRequestClusterSchema, PullRequestSchema } from '../../gen/gem_pb';
import { PullRequestList } from '../../PullRequestList';
import { Login } from '../../surfaces';
import { Icon, t3 } from '../../t3';
import { proposals } from '../fixtures';

// A profile is one gem server's worth of settings: whose PRs it lists (the login it runs as) and
// the GEM_SEARCH qualifiers narrowing them. Switching profiles swaps the whole cluster list.
type Profile = { id: string; name: string; login: string; search: string; clusters: PullRequestCluster[] };

const minutesAgo = (minutes: number) => timestampFromDate(new Date(Date.now() - minutes * 60_000));

const cluster = (
  name: string,
  updated: number,
  prs: { owner: string; repo: string; number: number; title: string; waiting?: number; approved?: boolean; checksPassing?: boolean; checksFailing?: boolean }[],
) =>
  create(PullRequestClusterSchema, {
    name,
    updatedAt: minutesAgo(updated),
    pullRequests: prs.map(({ owner, repo, number, title, waiting = 0, ...status }) =>
      create(PullRequestSchema, {
        repository: { owner, name: repo },
        number,
        title,
        url: `https://github.com/${owner}/${repo}/pull/${number}`,
        proposals: proposals.slice(0, waiting),
        ...status,
      }),
    ),
  });

// Made-up profiles: the same user's work split by organization, and a second GitHub account.
const profiles: Profile[] = [
  {
    id: 'livekit',
    name: 'LiveKit',
    login: 'u9g',
    search: 'org:livekit',
    clusters: [
      cluster('TTS retry fixes', 12, [
        { owner: 'livekit', repo: 'agents', number: 4821, title: 'feat(voice): retry TTS once on a dropped stream', waiting: 3, checksFailing: true },
        { owner: 'livekit', repo: 'agents-js', number: 1290, title: 'feat(voice): retry TTS once on a dropped stream', approved: true, checksPassing: true },
      ]),
      cluster('Room metadata typing', 190, [
        { owner: 'livekit', repo: 'protocol', number: 1147, title: 'Type room metadata updates', waiting: 1 },
      ]),
      cluster('Egress S3 region', 2 * 24 * 60, [
        { owner: 'livekit', repo: 'egress', number: 902, title: 'Respect the configured S3 region', approved: true, checksPassing: true },
      ]),
    ],
  },
  {
    id: 'prismarine',
    name: 'Prismarine',
    login: 'u9g',
    search: 'org:PrismarineJS org:ProtoDef-io',
    clusters: [
      cluster('Bedrock chunk decoding', 4, [
        { owner: 'PrismarineJS', repo: 'mineflayer', number: 4184, title: 'Decode Bedrock subchunks with palettes', waiting: 2, checksFailing: true },
        { owner: 'PrismarineJS', repo: 'prismarine-chunk', number: 288, title: 'Add Bedrock 1.21 subchunk format', waiting: 1 },
        { owner: 'ProtoDef-io', repo: 'node-protodef', number: 171, title: 'Support varint64 in compiled readers', approved: true, checksPassing: true },
      ]),
      cluster('Health regen events', 45, [
        { owner: 'PrismarineJS', repo: 'mineflayer', number: 4182, title: 'Emit health on regeneration ticks', waiting: 1, checksPassing: true },
      ]),
      cluster('Anvil region writes', 9 * 60, [
        { owner: 'PrismarineJS', repo: 'prismarine-provider-anvil', number: 96, title: 'Write regions atomically', approved: true, checksPassing: true },
      ]),
      cluster('', 3 * 24 * 60, [
        { owner: 'PrismarineJS', repo: 'mineflayer', number: 4176, title: 'Fix pathfinder stalls near doors' },
      ]),
    ],
  },
  {
    id: 'work',
    name: 'Wishblock',
    login: 'jason-wishblock',
    search: 'org:mc-chat-app',
    clusters: [
      cluster('Paper start timeout', 30, [
        { owner: 'mc-chat-app', repo: 'wishblock', number: 512, title: 'Wait for Paper to bind before reporting ready', waiting: 1, checksPassing: true },
      ]),
    ],
  },
];

function waiting(profile: Profile): number {
  return profile.clusters.reduce((sum, c) => sum + c.pullRequests.reduce((n, pr) => n + pr.proposals.length, 0), 0);
}

function failing(profile: Profile): boolean {
  return profile.clusters.some((c) => c.pullRequests.some((pr) => pr.checksFailing));
}

// Two letters on a tile tinted per profile, so profiles tell apart at a glance.
const tints = ['#3767E8', '#10B981', '#F59E0B', '#A855F7'];
function Avatar({ profile, size = 22 }: { profile: Profile; size?: number }) {
  const tint = tints[profiles.indexOf(profile) % tints.length];
  return (
    <View
      style={[
        styles.avatar,
        { width: size, height: size, borderRadius: size * 0.28, backgroundColor: `${tint}33`, borderColor: `${tint}66` },
      ]}
    >
      <Text style={[styles.avatarText, { fontSize: size * 0.42, color: tint }]}>{profile.name.slice(0, 2)}</Text>
    </View>
  );
}

function Count({ n }: { n: number }) {
  return n > 0 ? <Text style={styles.count}>{n}</Text> : null;
}

// Every design shares state: the profile shown and the cluster picked in it.
function useProfiles(initial = 1) {
  const [profile, setProfile] = useState(profiles[initial]);
  const [selectedUrl, setSelectedUrl] = useState<string>();
  const selected = profile.clusters.find((c) => c.pullRequests[0].url === selectedUrl) ?? profile.clusters[0];
  return {
    profile,
    selected,
    select: (c: PullRequestCluster) => setSelectedUrl(c.pullRequests[0].url),
    switchTo: (next: Profile) => {
      setProfile(next);
      setSelectedUrl(undefined);
    },
  };
}

// The menu every desktop design opens: each profile with its scope and how much waits on it.
function ProfileMenu({ current, onPick, style }: { current: Profile; onPick: (p: Profile) => void; style?: object }) {
  return (
    <View style={[styles.menu, style]}>
      {profiles.map((p) => (
        <MenuItem key={p.id} onPress={() => onPick(p)} selected={p === current}>
          <Avatar profile={p} />
          <View style={{ flex: 1, gap: 1 }}>
            <Text style={styles.menuName}>{p.name}</Text>
            <Text numberOfLines={1} style={styles.scope}>
              {p.login} · {p.search}
            </Text>
          </View>
          <Count n={waiting(p)} />
          <View style={{ width: 14 }}>{p === current && <Icon icon={Check} size={14} color={t3.foreground} />}</View>
        </MenuItem>
      ))}
      <View style={styles.menuDivider} />
      <MenuItem onPress={() => {}}>
        <View style={styles.addIcon}>
          <Icon icon={Plus} size={14} />
        </View>
        <Text style={styles.scope}>Add profile</Text>
      </MenuItem>
    </View>
  );
}

function MenuItem({ children, onPress, selected }: { children: ReactNode; onPress: () => void; selected?: boolean }) {
  const [hovered, setHovered] = useState(false);
  return (
    <Pressable
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.menuItem, selected && styles.menuItemSelected, hovered && styles.menuItemActive]}
    >
      {children}
    </Pressable>
  );
}

// The desktop app: sidebar of clusters, the picked cluster's PRs beside it. `top` sits above the
// cluster list, `rail` left of the sidebar.
function Desktop({
  state,
  top,
  rail,
}: {
  state: ReturnType<typeof useProfiles>;
  top: ReactNode;
  rail?: ReactNode;
}) {
  const { profile, selected, select } = state;
  return (
    <Login value={profile.login}>
      <View style={styles.container}>
        {rail}
        <View style={styles.sidebar}>
          {top}
          <ScrollView contentContainerStyle={styles.rows}>
            {profile.clusters.map((c) => (
              <ClusterRow key={c.pullRequests[0].url} cluster={c} selected={c === selected} onPress={() => select(c)} />
            ))}
          </ScrollView>
        </View>
        <ScrollView style={{ flex: 1 }} contentContainerStyle={styles.mainContent}>
          {selected && (
            <View style={styles.column}>
              <Text style={styles.title}>{selected.name || selected.pullRequests[0].title}</Text>
              <PullRequestList pullRequests={selected.pullRequests} />
            </View>
          )}
        </ScrollView>
      </View>
    </Login>
  );
}

// 1. Chip: the `author:` chip becomes `profile:`, a menu like `repos:`. Nothing new on screen.
function ChipDesign() {
  const state = useProfiles();
  const [open, setOpen] = useState(false);
  return (
    <Desktop
      state={state}
      top={
        <View style={{ zIndex: 1 }}>
          <View style={styles.filters}>
            <Text style={styles.filter}>server: online</Text>
            <Pressable onPress={() => setOpen(!open)}>
              <Text style={[styles.filter, open && styles.filterOpen]}>profile: {state.profile.name}</Text>
            </Pressable>
            <Text style={styles.filter}>repos: *</Text>
          </View>
          {open && (
            <ProfileMenu
              current={state.profile}
              style={{ position: 'absolute', top: 36, left: 90 }}
              onPick={(p) => {
                state.switchTo(p);
                setOpen(false);
              }}
            />
          )}
        </View>
      }
    />
  );
}

// 2. Header: a switcher heading the sidebar, like a workspace in Slack or Linear. Shows the scope
// under the name, so which PRs are listed is never a guess.
function HeaderDesign() {
  const state = useProfiles();
  const [open, setOpen] = useState(false);
  const { profile } = state;
  return (
    <Desktop
      state={state}
      top={
        <View style={{ zIndex: 1 }}>
          <Pressable onPress={() => setOpen(!open)} style={[styles.header, open && styles.headerOpen]}>
            <Avatar profile={profile} size={28} />
            <View style={{ flex: 1, gap: 1 }}>
              <Text style={styles.headerName}>{profile.name}</Text>
              <Text numberOfLines={1} style={styles.scope}>
                {profile.login} · {profile.search}
              </Text>
            </View>
            <Icon icon={ChevronsUpDown} size={14} />
          </Pressable>
          <View style={styles.filters}>
            <Text style={styles.filter}>server: online</Text>
            <Text style={styles.filter}>repos: *</Text>
          </View>
          {open && (
            <ProfileMenu
              current={profile}
              style={{ position: 'absolute', top: 54, left: 8, right: 8 }}
              onPick={(p) => {
                state.switchTo(p);
                setOpen(false);
              }}
            />
          )}
        </View>
      }
    />
  );
}

// 3. Rail: every profile always on screen, left of the sidebar, like Discord's servers. Other
// profiles' waiting proposals show as badges, so nothing waits unseen. One click switches.
function RailDesign() {
  const state = useProfiles();
  return (
    <Desktop
      state={state}
      rail={
        <View style={styles.rail}>
          {profiles.map((p) => (
            <RailItem key={p.id} profile={p} active={p === state.profile} onPress={() => state.switchTo(p)} />
          ))}
          <Pressable style={styles.railAdd} accessibilityLabel="Add profile">
            <Icon icon={Plus} size={16} />
          </Pressable>
        </View>
      }
      top={
        <View>
          <View style={styles.railTitle}>
            <Text style={styles.headerName}>{state.profile.name}</Text>
            <Text numberOfLines={1} style={styles.scope}>
              {state.profile.login} · {state.profile.search}
            </Text>
          </View>
          <View style={styles.filters}>
            <Text style={styles.filter}>server: online</Text>
            <Text style={styles.filter}>repos: *</Text>
          </View>
        </View>
      }
    />
  );
}

function RailItem({ profile, active, onPress }: { profile: Profile; active: boolean; onPress: () => void }) {
  const [hovered, setHovered] = useState(false);
  const n = waiting(profile);
  return (
    <Pressable
      accessibilityLabel={profile.name}
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={styles.railItem}
    >
      <View style={[styles.railMarker, active ? styles.railMarkerActive : hovered && styles.railMarkerHover]} />
      <View style={{ opacity: active || hovered ? 1 : 0.7 }}>
        <Avatar profile={profile} size={34} />
      </View>
      {!active && n > 0 && (
        <View style={[styles.badge, failing(profile) && styles.badgeFailing]}>
          <Text style={styles.badgeText}>{n}</Text>
        </View>
      )}
    </Pressable>
  );
}

// 4. Phone: the large title is the profile's name; tapping it opens a sheet of profiles.
function PhoneDesign() {
  const state = useProfiles();
  const [open, setOpen] = useState(false);
  const { profile } = state;
  return (
    <Login value={profile.login}>
      <View style={styles.phoneFrame}>
        <View style={styles.phone}>
          <Pressable onPress={() => setOpen(true)} style={styles.largeTitleRow}>
            <Text style={styles.largeTitle}>{profile.name}</Text>
            <Icon icon={ChevronDown} size={22} color={t3.foreground} />
          </Pressable>
          <Text style={[styles.scope, { paddingHorizontal: 16 }]}>
            {profile.login} · {profile.search}
          </Text>
          <View style={styles.filters}>
            <Text style={styles.filter}>server: online</Text>
            <Text style={styles.filter}>repos: *</Text>
          </View>
          <ScrollView>
            {profile.clusters.map((c) => (
              <PhoneClusterRow key={c.pullRequests[0].url} cluster={c} onPress={() => {}} />
            ))}
          </ScrollView>
          {open && (
            <>
              <Pressable style={styles.scrim} onPress={() => setOpen(false)} />
              <View style={styles.sheet}>
                <View style={styles.grabber} />
                <Text style={styles.sheetTitle}>Profiles</Text>
                {profiles.map((p) => (
                  <Pressable
                    key={p.id}
                    style={styles.sheetRow}
                    onPress={() => {
                      state.switchTo(p);
                      setOpen(false);
                    }}
                  >
                    <Avatar profile={p} size={34} />
                    <View style={{ flex: 1, gap: 2 }}>
                      <Text style={styles.sheetName}>{p.name}</Text>
                      <Text numberOfLines={1} style={styles.scope}>
                        {p.login} · {p.search}
                      </Text>
                    </View>
                    <Count n={waiting(p)} />
                    <View style={{ width: 18 }}>{p === profile && <Icon icon={Check} size={18} color={t3.primary} />}</View>
                  </Pressable>
                ))}
                <Pressable style={styles.sheetRow}>
                  <View style={[styles.addIcon, { width: 34, height: 34 }]}>
                    <Icon icon={Plus} size={16} />
                  </View>
                  <Text style={styles.sheetName}>Add profile</Text>
                </Pressable>
              </View>
            </>
          )}
        </View>
      </View>
    </Login>
  );
}

const meta: Meta = { title: 'Profiles', parameters: { layout: 'fullscreen' } };
export default meta;

export const Chip: StoryObj = { name: '1. Filter chip', render: () => <ChipDesign /> };
export const Header: StoryObj = { name: '2. Sidebar header', render: () => <HeaderDesign /> };
export const Rail: StoryObj = { name: '3. Rail', render: () => <RailDesign /> };
export const Phone: StoryObj = { name: '4. Phone sheet', render: () => <PhoneDesign /> };

const styles = StyleSheet.create({
  container: {
    flex: 1,
    minHeight: '100vh',
    flexDirection: 'row',
    backgroundColor: t3.background,
  } as object,
  sidebar: {
    width: 320,
    backgroundColor: '#1D2630',
    borderRightWidth: 1,
    borderRightColor: t3.border,
  },
  rows: {
    paddingHorizontal: 8,
    paddingBottom: 8,
  },
  mainContent: {
    alignItems: 'center',
    paddingHorizontal: 24,
    paddingVertical: 32,
  },
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 16,
  },
  title: {
    color: t3.foreground,
    fontSize: 20,
    fontWeight: '600',
  },
  filters: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 6,
    padding: 10,
  },
  filter: {
    color: t3.foreground,
    fontSize: 11,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  filterOpen: {
    backgroundColor: 'rgba(255, 255, 255, 0.12)',
  },
  scope: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
  },
  avatar: {
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
  },
  avatarText: {
    fontWeight: '700',
  },
  count: {
    color: '#8FA9F5',
    fontSize: 10,
    fontWeight: '600',
    paddingHorizontal: 6,
    lineHeight: 18,
    borderRadius: 4,
    borderWidth: 1,
    borderColor: 'rgba(55, 103, 232, 0.4)',
    backgroundColor: 'rgba(55, 103, 232, 0.1)',
    fontVariant: ['tabular-nums'],
  },
  menu: {
    width: 300,
    padding: 4,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
    backgroundColor: t3.background,
    shadowColor: '#000',
    shadowOpacity: 0.4,
    shadowRadius: 16,
    shadowOffset: { width: 0, height: 8 },
  },
  menuItem: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 8,
    paddingVertical: 6,
    borderRadius: 6,
  },
  menuItemSelected: {
    backgroundColor: '#2E3946',
  },
  menuItemActive: {
    backgroundColor: '#3A4757',
  },
  menuName: {
    color: t3.foreground,
    fontSize: 13,
  },
  menuDivider: {
    height: 1,
    marginVertical: 4,
    backgroundColor: t3.border,
  },
  addIcon: {
    width: 22,
    height: 22,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    margin: 8,
    marginBottom: 0,
    padding: 8,
    borderRadius: 8,
  },
  headerOpen: {
    backgroundColor: 'rgba(255, 255, 255, 0.05)',
  },
  headerName: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
  },
  rail: {
    width: 56,
    alignItems: 'center',
    gap: 8,
    paddingVertical: 12,
    backgroundColor: '#151C24',
    borderRightWidth: 1,
    borderRightColor: t3.border,
  },
  railItem: {
    width: 56,
    alignItems: 'center',
  },
  railMarker: {
    position: 'absolute',
    left: 0,
    top: 13,
    width: 3,
    height: 8,
    borderTopRightRadius: 3,
    borderBottomRightRadius: 3,
  },
  railMarkerHover: {
    backgroundColor: t3.muted,
  },
  railMarkerActive: {
    top: 5,
    height: 24,
    backgroundColor: t3.foreground,
  },
  badge: {
    position: 'absolute',
    right: 6,
    bottom: -4,
    minWidth: 16,
    height: 16,
    paddingHorizontal: 4,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 8,
    borderWidth: 2,
    borderColor: '#151C24',
    backgroundColor: t3.primary,
  },
  badgeFailing: {
    backgroundColor: '#DC4C4C',
  },
  badgeText: {
    color: '#FFFFFF',
    fontSize: 9,
    fontWeight: '700',
  },
  railAdd: {
    width: 34,
    height: 34,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  railTitle: {
    gap: 2,
    paddingHorizontal: 18,
    paddingTop: 14,
  },
  phoneFrame: {
    minHeight: '100vh',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 24,
    backgroundColor: '#11171E',
  } as object,
  phone: {
    width: 390,
    height: 760,
    overflow: 'hidden',
    borderRadius: 32,
    borderWidth: 1,
    borderColor: '#2E3946',
    backgroundColor: t3.background,
  },
  largeTitleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 16,
    paddingTop: 16,
  },
  largeTitle: {
    color: t3.foreground,
    fontSize: 28,
    fontWeight: '700',
  },
  scrim: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.5)',
  },
  sheet: {
    position: 'absolute',
    left: 0,
    right: 0,
    bottom: 0,
    paddingBottom: 24,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    backgroundColor: '#1D2630',
  },
  grabber: {
    alignSelf: 'center',
    width: 36,
    height: 5,
    marginTop: 8,
    borderRadius: 3,
    backgroundColor: 'rgba(255, 255, 255, 0.2)',
  },
  sheetTitle: {
    color: t3.foreground,
    fontSize: 17,
    fontWeight: '600',
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  sheetRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    minHeight: 60,
    paddingHorizontal: 16,
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
  sheetName: {
    color: t3.foreground,
    fontSize: 16,
  },
});
