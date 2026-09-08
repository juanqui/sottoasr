<script lang="ts">
  interface Props { open: boolean; title: string; message: string; confirmLabel: string; cancelLabel?: string; secondaryLabel?: string; busy?: boolean; confirmDisabled?: boolean; onconfirm: () => void; oncancel: () => void; onsecondary?: () => void; }
  let { open, title, message, confirmLabel, cancelLabel = 'Cancel', secondaryLabel, busy = false, confirmDisabled = false, onconfirm, oncancel, onsecondary }: Props = $props();
  const id = $props.id();
  let dialog: HTMLDialogElement;
  let cancelButton: HTMLButtonElement;
  $effect(() => {
    if (open) { dialog.showModal(); cancelButton.focus(); }
    else if (dialog.open) dialog.close();
  });
</script>
<dialog bind:this={dialog} aria-labelledby={`${id}-title`} aria-describedby={`${id}-message`} oncancel={(event) => { event.preventDefault(); if (!busy) oncancel(); }}>
  <h2 id={`${id}-title`}>{title}</h2>
  <p id={`${id}-message`}>{message}</p>
  <div class="dialog-actions">
    <button bind:this={cancelButton} type="button" onclick={oncancel} disabled={busy}>{cancelLabel}</button>
    {#if secondaryLabel}<button type="button" onclick={onsecondary} disabled={busy}>{secondaryLabel}</button>{/if}
    <button class="confirm" type="button" onclick={onconfirm} disabled={busy || confirmDisabled}>{busy ? 'Please wait…' : confirmLabel}</button>
  </div>
</dialog>
<style>
  dialog { box-sizing:border-box; max-width:calc(100vw - 40px); width:420px; color:var(--text); background:var(--bg); border:1px solid var(--border); border-radius:14px; padding:24px; box-shadow:0 20px 60px #0007; }
  dialog::backdrop { background:#0007; }
  h2 { margin:0 0 10px; font-size:18px; color:var(--text-bright); }
  p { font-size:13px; line-height:1.6; margin:0 0 22px; }
  .dialog-actions { display:flex; gap:8px; flex-wrap:wrap; justify-content:flex-end; }
  button { border:1px solid var(--border); border-radius:8px; background:var(--card-bg); color:var(--text); padding:8px 11px; font:inherit; font-size:12px; cursor:pointer; }
  button.confirm { background:var(--accent); color:white; border-color:var(--accent); }
  button:disabled { opacity:.5; cursor:default; }
</style>
