<script lang="ts">
  // A group of settings under one heading, its (i) and a rule, so the Captions panel and Settings
  // separate their groups alike.
  import type { Snippet } from "svelte";
  import InfoTip from "./InfoTip.svelte";

  let { title, info, children }: { title: string; info?: string; children: Snippet } = $props();
</script>

<section>
  <div class="head">
    <h3>{title}</h3>
    {#if info}<InfoTip label={title} text={info} />{/if}
  </div>
  {@render children()}
</section>

<style>
  section {
    margin-top: 18px;
  }
  section:first-child {
    margin-top: 8px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-bottom: 6px;
    margin-bottom: 4px;
    border-bottom: 1px solid var(--line);
  }
  h3 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
</style>
