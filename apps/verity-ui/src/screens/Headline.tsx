import type { Verdict } from "../checks.ts";

/** The verdict sentence; the part after "but" carries the mark's colour. */
export function Headline({ id, v }: { id?: string; v: Verdict }) {
  const [before, after] = v.headline.split(/ but /);
  return (
    <>
      <h1 id={id}>
        {after ? (
          <>
            {before} but <span className={`hl-${v.mark}`}>{after}</span>
          </>
        ) : (
          v.headline
        )}
      </h1>
      <p>{v.detail}</p>
    </>
  );
}
