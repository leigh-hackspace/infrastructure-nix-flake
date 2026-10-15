import type { ParentProps } from 'solid-js';

export function Card(props: ParentProps<{ title: string }>) {
  return (
    <section class="card">
      <h2>{props.title}</h2>
      {props.children}
    </section>
  );
}

export function Pill(props: ParentProps<{ kind: 'good' | 'bad' | 'wait' }>) {
  return <span class={`pill ${props.kind}`}>{props.children}</span>;
}
