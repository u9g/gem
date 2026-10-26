import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { Board } from '../../Board';
import { PhoneClusterRow } from '../../ClusterRow';
import { type PullRequestCluster, PullRequestClusterSchema, PullRequestSchema } from '../../gen/gem_pb';
import { ProfileChip, ProfileForm, ProfileSheet, ProfileTitle } from '../../Profiles';
import { PullRequestList } from '../../PullRequestList';
import { type Profile, type Project } from '../../server';
import { Login } from '../../surfaces';
import { t3 } from '../../t3';
import { proposals } from '../fixtures';

// Switching profiles connects to another gem server, so the whole cluster list changes.
const minutesAgo = (minutes: number) => timestampFromDate(new Date(Date.now() - minutes * 60_000));

const cluster = (
  name: string,
  updated: number,
  prs: { owner: string; repo: string; number: number; title: string; waiting?: number; approved?: boolean; checksPassing?: boolean; checksFailing?: boolean }[],
) =>
  create(PullRequestClusterSchema, {
    name,
    updatedAt: minutesAgo(updated),
    mergeable: prs.every((pr) => pr.approved && pr.checksPassing),
    pullRequests: prs.map(({ owner, repo, number, title, waiting = 0, ...status }) =>
      create(PullRequestSchema, {
        repository: { owner, name: repo },
        number,
        title,
        url: `https://github.com/${owner}/${repo}/pull/${number}`,
        proposals: proposals.slice(0, waiting),
        mergeable: Boolean(status.approved && status.checksPassing),
        ...status,
      }),
    ),
  });

// Made-up profiles: the same user's work split by organization, and a second GitHub account,
// each with the clusters its server would list.
const fixtures: (Omit<Profile, keyof Project | 'room'> & { clusters: PullRequestCluster[] })[] = [
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

const project: Project = { url: 'wss://example.livekit.cloud', apiKey: 'key', apiSecret: 'secret' };
const profiles: Profile[] = fixtures.map(({ clusters, ...profile }) => ({ ...project, ...profile, room: 'gem' }));
const clustersOf = (profile: Profile) => fixtures.find((f) => f.id === profile.id)!.clusters;

function useProfiles() {
  const [profile, setProfile] = useState(profiles[1]);
  const [selectedUrl, setSelectedUrl] = useState<string>();
  const clusters = clustersOf(profile);
  return {
    profile,
    clusters,
    selectedUrl,
    selected: clusters.find((c) => c.pullRequests[0].url === selectedUrl),
    select: (c?: PullRequestCluster) => setSelectedUrl(c?.pullRequests[0].url),
    switchTo: (id: string) => {
      setProfile(profiles.find((p) => p.id === id)!);
      setSelectedUrl(undefined);
    },
  };
}

// The desktop app: the `profile:` chip among the filters over the board, and the picked cluster's
// PRs in the drawer.
function Desktop() {
  const { profile, clusters, selectedUrl, selected, select, switchTo } = useProfiles();
  return (
    <Login value={profile.login!}>
      <View style={styles.container}>
        <View style={styles.filters}>
          <Text style={styles.filter}>server: online</Text>
          <ProfileChip profiles={profiles} profile={profile} onSelect={switchTo} onAdd={() => {}} />
          <Text style={styles.filter}>repos: *</Text>
        </View>
        <Board
          clusters={clusters}
          investigations={[]}
          openUrl={selectedUrl}
          onOpenCluster={select}
          onOpenInvestigation={() => {}}
          onClose={() => select(undefined)}
        >
          {selected && (
            <View style={styles.column}>
              <Text style={styles.title}>{selected.name || selected.pullRequests[0].title}</Text>
              <PullRequestList pullRequests={selected.pullRequests} />
            </View>
          )}
        </Board>
      </View>
    </Login>
  );
}

// The phone's cluster list, titled by the profile; tapping the title opens the sheet.
function Phone() {
  const { profile, clusters, switchTo } = useProfiles();
  return (
    <Login value={profile.login!}>
      <View style={styles.phoneFrame}>
        <View style={styles.phone}>
          <ProfileTitle profiles={profiles} profile={profile} onSelect={switchTo} onAdd={() => {}} />
          <View style={styles.filters}>
            <Text style={styles.filter}>server: online</Text>
            <Text style={styles.filter}>repos: *</Text>
          </View>
          <ScrollView>
            {clusters.map((c) => (
              <PhoneClusterRow key={c.pullRequests[0].url} cluster={c} onPress={() => {}} />
            ))}
          </ScrollView>
        </View>
      </View>
    </Login>
  );
}

const meta: Meta = { title: 'Profiles', parameters: { layout: 'fullscreen' } };
export default meta;

export const Chip: StoryObj = { name: 'Filter chip', render: () => <Desktop /> };
export const PhoneTitle: StoryObj = { name: 'Phone title', render: () => <Phone /> };
export const Sheet: StoryObj = {
  name: 'Phone sheet',
  render: () => (
    <View style={styles.phoneFrame}>
      <ProfileSheet open profiles={profiles} profile={profiles[1]} onClose={() => {}} onSelect={() => {}} onAdd={() => {}} />
    </View>
  ),
};
export const Add: StoryObj = { name: 'Add profile', render: () => <ProfileForm onSave={() => {}} onCancel={() => {}} /> };
export const Change: StoryObj = {
  name: 'Change profile',
  render: () => <ProfileForm editing={profiles[1]} onSave={() => {}} onCancel={() => {}} onRemove={() => {}} />,
};

const styles = StyleSheet.create({
  container: {
    flex: 1,
    height: '100vh',
    backgroundColor: t3.background,
  } as object,
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
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  filter: {
    color: t3.foreground,
    fontSize: 11,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
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
});
