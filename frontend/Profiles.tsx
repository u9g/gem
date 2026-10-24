import * as Select from '@rn-primitives/select';
import { Check, ChevronDown, Plus } from 'lucide-react';
import { useState } from 'react';
import { Modal, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import type { Profile } from './server';
import { Icon, t3 } from './t3';

// Only the selected profile is connected, so the others show what their servers last reported.
function scope(profile: Profile): string {
  return [profile.login, profile.search].filter(Boolean).join(' · ');
}

// Two letters on a tile tinted by the profile's place in the list, so profiles tell apart at a
// glance.
const tints = ['#3767E8', '#10B981', '#F59E0B', '#A855F7', '#EC4899'];
function Avatar({ profiles, profile, size }: { profiles: Profile[]; profile: Profile; size: number }) {
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

const ADD = '+';

// The `profile:` filter chip: a menu of profiles, each with its scope, then Add profile.
export function ProfileChip({
  profiles,
  profile,
  onSelect,
  onAdd,
}: {
  profiles: Profile[];
  profile: Profile;
  onSelect: (id: string) => void;
  onAdd: () => void;
}) {
  const [highlighted, setHighlighted] = useState<string>();
  // On web the Item is Radix's DOM element, so its style must be one plain object of CSS
  // properties. Radix focuses whichever item the pointer or keyboard highlights.
  const item = (value: string, selected = false) => ({
    onFocus: () => setHighlighted(value),
    onBlur: () => setHighlighted(undefined),
    style: StyleSheet.flatten([
      styles.menuItem,
      selected && styles.menuItemSelected,
      highlighted === value && styles.menuItemActive,
    ]),
  });
  return (
    <Select.Root
      value={{ value: profile.id, label: profile.name }}
      onValueChange={(option) => (option?.value === ADD ? onAdd() : option && onSelect(option.value))}
    >
      <Select.Trigger>
        <Text style={styles.filter}>profile: {profile.name}</Text>
      </Select.Trigger>
      <Select.Portal>
        <Select.Content position="popper" sideOffset={4} style={styles.menu}>
          <Select.Viewport>
            {profiles.map((p) => (
              <Select.Item key={p.id} value={p.id} label={p.name} {...item(p.id, p === profile)}>
                <Avatar profiles={profiles} profile={p} size={22} />
                <View style={{ flex: 1, gap: 1 }}>
                  <Select.ItemText style={styles.menuName} />
                  {scope(p) !== '' && (
                    <Text numberOfLines={1} style={styles.scope}>
                      {scope(p)}
                    </Text>
                  )}
                </View>
                <View style={{ width: 14 }}>{p === profile && <Icon icon={Check} size={14} color={t3.foreground} />}</View>
              </Select.Item>
            ))}
            <View style={styles.menuDivider} />
            <Select.Item value={ADD} label="Add profile" {...item(ADD)}>
              <View style={[styles.addIcon, { width: 22, height: 22 }]}>
                <Icon icon={Plus} size={14} />
              </View>
              <Select.ItemText style={styles.scope} />
            </Select.Item>
          </Select.Viewport>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}

// The phone's large title: the selected profile's name, opening a sheet of profiles.
export function ProfileTitle({
  profiles,
  profile,
  onSelect,
  onAdd,
}: {
  profiles: Profile[];
  profile: Profile;
  onSelect: (id: string) => void;
  onAdd: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Pressable accessibilityLabel="Switch profile" onPress={() => setOpen(true)} style={styles.titleRow}>
        <Text style={styles.largeTitle}>{profile.name}</Text>
        <Icon icon={ChevronDown} size={22} color={t3.foreground} />
      </Pressable>
      {scope(profile) !== '' && <Text style={[styles.scope, { paddingHorizontal: 16 }]}>{scope(profile)}</Text>}
      <ProfileSheet
        open={open}
        profiles={profiles}
        profile={profile}
        onClose={() => setOpen(false)}
        onSelect={(id) => {
          setOpen(false);
          onSelect(id);
        }}
        onAdd={() => {
          setOpen(false);
          onAdd();
        }}
      />
    </>
  );
}

export function ProfileSheet({
  open,
  profiles,
  profile,
  onClose,
  onSelect,
  onAdd,
}: {
  open: boolean;
  profiles: Profile[];
  profile: Profile;
  onClose: () => void;
  onSelect: (id: string) => void;
  onAdd: () => void;
}) {
  return (
    <Modal transparent animationType="fade" visible={open} onRequestClose={onClose}>
      <Pressable accessibilityLabel="Close" style={styles.scrim} onPress={onClose} />
      <View style={styles.sheet}>
        <View style={styles.grabber} />
        <Text style={styles.sheetTitle}>Profiles</Text>
        {profiles.map((p) => (
          <Pressable key={p.id} style={styles.sheetRow} onPress={() => onSelect(p.id)}>
            <Avatar profiles={profiles} profile={p} size={34} />
            <View style={{ flex: 1, gap: 2 }}>
              <Text style={styles.sheetName}>{p.name}</Text>
              {scope(p) !== '' && (
                <Text numberOfLines={1} style={styles.scope}>
                  {scope(p)}
                </Text>
              )}
            </View>
            <View style={{ width: 18 }}>{p === profile && <Icon icon={Check} size={18} color={t3.primary} />}</View>
          </Pressable>
        ))}
        <Pressable style={styles.sheetRow} onPress={onAdd}>
          <View style={[styles.addIcon, { width: 34, height: 34 }]}>
            <Icon icon={Plus} size={16} />
          </View>
          <Text style={styles.sheetName}>Add profile</Text>
        </Pressable>
      </View>
    </Modal>
  );
}

// A gem server to add, or the selected one to change: its name, the LiveKit project it is
// configured with (LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET) and its GEM_ROOM.
export function ProfileForm({
  editing,
  onSave,
  onCancel,
  onRemove,
}: {
  editing?: Profile;
  onSave: (profile: Profile) => void;
  // Absent when there is no profile to go back to.
  onCancel?: () => void;
  // Present for a saved profile.
  onRemove?: () => void;
}) {
  const [profile, setProfile] = useState<Profile>(
    () => editing ?? { id: crypto.randomUUID(), name: '', url: '', apiKey: '', apiSecret: '', room: '' },
  );
  const field = (key: 'name' | 'url' | 'apiKey' | 'apiSecret' | 'room', placeholder: string) => (
    <TextInput
      style={styles.input}
      placeholder={placeholder}
      placeholderTextColor="#6B6A70"
      secureTextEntry={key === 'apiSecret'}
      autoCapitalize="none"
      value={profile[key]}
      onChangeText={(value) => setProfile({ ...profile, [key]: value })}
    />
  );
  const save = () => onSave({ ...profile, name: profile.name.trim() || 'gem', room: profile.room.trim() });
  return (
    <View style={styles.form}>
      <Text style={styles.formText}>{editing ? `Change ${editing.name}` : 'Add a gem server'}</Text>
      {field('name', 'Name, e.g. LiveKit')}
      {field('url', 'LIVEKIT_URL')}
      {field('apiKey', 'LIVEKIT_API_KEY')}
      {field('apiSecret', 'LIVEKIT_API_SECRET')}
      {field('room', 'GEM_ROOM')}
      <View style={styles.formButtons}>
        {onRemove && (
          <Pressable style={styles.formButton} onPress={onRemove}>
            <Text style={[styles.formText, { color: t3.deletion }]}>Remove</Text>
          </Pressable>
        )}
        {onCancel && (
          <Pressable style={styles.formButton} onPress={onCancel}>
            <Text style={styles.formText}>Cancel</Text>
          </Pressable>
        )}
        <Pressable style={[styles.formButton, styles.formButtonPrimary]} onPress={save}>
          <Text style={styles.formText}>{editing ? 'Save' : 'Connect'}</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  filter: {
    color: t3.foreground,
    fontSize: 11,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
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
  addIcon: {
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  menu: {
    width: 300,
    padding: 4,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
    backgroundColor: t3.background,
  },
  // Reaches the DOM unconverted (see ProfileChip), so only properties CSS also has.
  menuItem: {
    display: 'flex',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingLeft: 8,
    paddingRight: 8,
    paddingTop: 6,
    paddingBottom: 6,
    borderRadius: 6,
    // Chrome's focus ring (outline-style: auto) ignores outline-width; a solid 0px one draws nothing.
    outlineStyle: 'solid',
    outlineWidth: 0,
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
  titleRow: {
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
    flex: 1,
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
  form: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 10,
    backgroundColor: t3.background,
  },
  input: {
    width: 320,
    color: t3.foreground,
    fontSize: 14,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 8,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  formButtons: {
    flexDirection: 'row',
    gap: 8,
  },
  formButton: {
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderRadius: 8,
  },
  formButtonPrimary: {
    backgroundColor: 'rgba(255, 255, 255, 0.1)',
  },
  formText: {
    color: t3.foreground,
    fontSize: 14,
  },
});
