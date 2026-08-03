---
name: react-antipatterns
description: >-
  React コンポーネントや custom Hook をレビュー、生成、リファクタするときに使うスキル。公式 React docs
  と信頼できる著者の記事を根拠に、Effect の乱用、状態設計ミス、state ownership の崩れ、render
  の不純化、ref/context/custom Hook の誤用、Rules of Hooks 違反、過剰な memoization
  を避け、長期保守しやすい実装に寄せる。
---
# React Anti-Patterns With Examples

このファイルは、React のアンチパターンを `Don't / Do / Why / References` で説明するための具体例集です。
レビュー時は、対象コードに一番近い例を選んでから説明してください。

## 1. 派生値を `Effect` で state 化しない

Don't:

```tsx
function TodoList({ todos, filter }: Props) {
  const [visibleTodos, setVisibleTodos] = useState<Todo[]>([]);

  useEffect(() => {
    setVisibleTodos(getFilteredTodos(todos, filter));
  }, [todos, filter]);

  return <List items={visibleTodos} />;
}
```

Do:

```tsx
function TodoList({ todos, filter }: Props) {
  const visibleTodos = getFilteredTodos(todos, filter);
  return <List items={visibleTodos} />;
}
```

重い計算だけ:

```tsx
function TodoList({ todos, filter }: Props) {
  const visibleTodos = useMemo(
    () => getFilteredTodos(todos, filter),
    [todos, filter]
  );

  return <List items={visibleTodos} />;
}
```

Why:
- `Effect -> setState` は余計な render を 1 回増やす
- source state と derived state がズレやすい

References:
- https://react.dev/learn/you-might-not-need-an-effect
- https://overreacted.io/a-complete-guide-to-useeffect/

## 2. user event の処理を `Effect` に逃がさない

Don't:

```tsx
function BuyButton({ product }: { product: Product }) {
  const [isBuying, setIsBuying] = useState(false);

  useEffect(() => {
    if (!isBuying) return;

    post("/api/buy", { productId: product.id });
    showToast(`${product.name} was added to the cart`);
  }, [isBuying, product]);

  return <button onClick={() => setIsBuying(true)}>Buy</button>;
}
```

Do:

```tsx
function BuyButton({ product }: { product: Product }) {
  async function handleBuy() {
    await post("/api/buy", { productId: product.id });
    showToast(`${product.name} was added to the cart`);
  }

  return <button onClick={handleBuy}>Buy</button>;
}
```

Why:
- 処理の起点が click なのに、state 変化に隠れてしまう
- 再実行条件が読みにくくなり、順序バグを招く

References:
- https://react.dev/learn/you-might-not-need-an-effect
- https://react.dev/learn/separating-events-from-effects
- https://overreacted.io/a-complete-guide-to-useeffect/

## 3. prop 変更に合わせて `Effect` で reset しない

Don't:

```tsx
function ProfilePage({ userId }: { userId: string }) {
  const [comment, setComment] = useState("");

  useEffect(() => {
    setComment("");
  }, [userId]);

  return <CommentEditor value={comment} onChange={setComment} />;
}
```

Do:

```tsx
function ProfilePage({ userId }: { userId: string }) {
  return <Profile key={userId} userId={userId} />;
}

function Profile({ userId }: { userId: string }) {
  const [comment, setComment] = useState("");

  return <CommentEditor value={comment} onChange={setComment} />;
}
```

Why:
- 古い state で一度 render してから reset する形になり不自然
- reset したい境界が code から読み取りにくい

References:
- https://react.dev/learn/you-might-not-need-an-effect
- https://react.dev/learn/preserving-and-resetting-state
- https://kentcdodds.com/blog/understanding-reacts-key-prop

## 4. object の二重保持をやめて ID だけ持つ

Don't:

```tsx
function Mailbox({ messages }: { messages: Message[] }) {
  const [selectedMessage, setSelectedMessage] = useState<Message | null>(null);

  useEffect(() => {
    if (!selectedMessage) return;

    setSelectedMessage(
      messages.find((message) => message.id === selectedMessage.id) ?? null
    );
  }, [messages, selectedMessage]);

  return (
    <>
      <MessageList messages={messages} onSelect={setSelectedMessage} />
      <MessageView message={selectedMessage} />
    </>
  );
}
```

Do:

```tsx
function Mailbox({ messages }: { messages: Message[] }) {
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const selectedMessage =
    messages.find((message) => message.id === selectedId) ?? null;

  return (
    <>
      <MessageList messages={messages} onSelect={(message) => setSelectedId(message.id)} />
      <MessageView message={selectedMessage} />
    </>
  );
}
```

Why:
- array と selected object の二重保持は簡単にズレる
- `Effect` で後追い同期が必要になる時点で state shape が怪しい

References:
- https://react.dev/learn/choosing-the-state-structure
- https://react.dev/learn/you-might-not-need-an-effect

## 5. shared state を child ごとに持たない

Don't:

```tsx
function Accordion() {
  return (
    <>
      <Panel title="About" body="React lets you compose UIs from components." />
      <Panel title="Etymology" body="The word comes from the instrument accordion." />
    </>
  );
}

function Panel({ title, body }: { title: string; body: string }) {
  const [isActive, setIsActive] = useState(false);

  return (
    <section>
      <h3>{title}</h3>
      {isActive && <p>{body}</p>}
      <button onClick={() => setIsActive(!isActive)}>
        {isActive ? "Hide" : "Show"}
      </button>
    </section>
  );
}
```

Do:

```tsx
function Accordion() {
  const [activeIndex, setActiveIndex] = useState<number | null>(null);

  return (
    <>
      <Panel
        title="About"
        body="React lets you compose UIs from components."
        isActive={activeIndex === 0}
        onShow={() => setActiveIndex(0)}
      />
      <Panel
        title="Etymology"
        body="The word comes from the instrument accordion."
        isActive={activeIndex === 1}
        onShow={() => setActiveIndex(1)}
      />
    </>
  );
}

function Panel({
  title,
  body,
  isActive,
  onShow,
}: {
  title: string;
  body: string;
  isActive: boolean;
  onShow: () => void;
}) {
  return (
    <section>
      <h3>{title}</h3>
      {isActive ? <p>{body}</p> : <button onClick={onShow}>Show</button>}
    </section>
  );
}
```

Why:
- sibling 間で同期すべき state を local に持つと整合性が崩れる
- owner が分散すると仕様変更に弱くなる

References:
- https://react.dev/learn/sharing-state-between-components

## 6. render を不純にしない

Don't:

```tsx
function Clock() {
  const [time, setTime] = useState(Date.now());

  setTime(Date.now());

  return <span>{new Date(time).toLocaleTimeString()}</span>;
}
```

Do:

```tsx
function Clock() {
  const [time, setTime] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setTime(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  return <span>{new Date(time).toLocaleTimeString()}</span>;
}
```

Why:
- render 中の `setState` や side effect は replay / interrupt と相性が悪い
- Strict Mode や hydration で破綻しやすい

References:
- https://react.dev/learn/keeping-components-pure
- https://react.dev/reference/rules/components-and-hooks-must-be-pure
- https://react.dev/reference/eslint-plugin-react-hooks/lints/set-state-in-render

## 7. state と props を mutate しない

Don't:

```tsx
function SortedList({ items }: { items: string[] }) {
  items.sort();
  return (
    <ul>
      {items.map((item) => (
        <li key={item}>{item}</li>
      ))}
    </ul>
  );
}
```

Do:

```tsx
function SortedList({ items }: { items: string[] }) {
  const sortedItems = [...items].sort();

  return (
    <ul>
      {sortedItems.map((item) => (
        <li key={item}>{item}</li>
      ))}
    </ul>
  );
}
```

Why:
- prop や state を mutate すると local reasoning が壊れる
- memoization や再 render 判定も信用しづらくなる

References:
- https://react.dev/reference/rules/components-and-hooks-must-be-pure
- https://react.dev/learn/updating-arrays-in-state
- https://react.dev/learn/updating-objects-in-state

## 8. Hooks を条件分岐の中で呼ばない

Don't:

```tsx
function ChatRoom({ roomId }: { roomId: string | null }) {
  if (roomId) {
    useEffect(() => {
      const connection = createConnection(roomId);
      connection.connect();
      return () => connection.disconnect();
    }, [roomId]);
  }

  return <div>{roomId ? "Connected" : "Select a room"}</div>;
}
```

Do:

```tsx
function ChatRoom({ roomId }: { roomId: string | null }) {
  useEffect(() => {
    if (!roomId) return;

    const connection = createConnection(roomId);
    connection.connect();
    return () => connection.disconnect();
  }, [roomId]);

  return <div>{roomId ? "Connected" : "Select a room"}</div>;
}
```

Why:
- Hook call order が render ごとに変わると state と effect の対応が壊れる

References:
- https://react.dev/reference/rules/rules-of-hooks
- https://kentcdodds.com/blog/react-hooks-pitfalls

## 9. render に必要な値を `ref` に逃がさない

Don't:

```tsx
function Counter() {
  const countRef = useRef(0);

  function handleClick() {
    countRef.current += 1;
  }

  return <button onClick={handleClick}>{countRef.current}</button>;
}
```

Do:

```tsx
function Counter() {
  const [count, setCount] = useState(0);

  function handleClick() {
    setCount((c) => c + 1);
  }

  return <button onClick={handleClick}>{count}</button>;
}
```

Why:
- `ref.current` の変更では再 render されない
- UI 表示に必要な値を ref に置くと表示と内部状態がズレる

References:
- https://react.dev/learn/referencing-values-with-refs
- https://overreacted.io/a-complete-guide-to-useeffect/

## 10. context を props 回避の万能策にしない

Don't:

```tsx
const UserNameContext = createContext<string | null>(null);

function Page({ userName }: { userName: string }) {
  return (
    <UserNameContext.Provider value={userName}>
      <Header />
    </UserNameContext.Provider>
  );
}

function Header() {
  return <Toolbar />;
}

function Toolbar() {
  const userName = useContext(UserNameContext);
  return <h1>{userName}</h1>;
}
```

Do:

```tsx
function Page({ userName }: { userName: string }) {
  return <Header userName={userName} />;
}

function Header({ userName }: { userName: string }) {
  return <Toolbar userName={userName} />;
}

function Toolbar({ userName }: { userName: string }) {
  return <h1>{userName}</h1>;
}
```

Why:
- 近い tree での受け渡しまで context 化すると data flow が暗黙になる
- 依存関係の追跡が難しくなる

References:
- https://react.dev/learn/passing-data-deeply-with-context
- https://react.dev/learn/scaling-up-with-reducer-and-context
- https://kentcdodds.com/blog/how-to-use-react-context-effectively

## 11. custom Hook は lifecycle 隠蔽ではなく logic 共有のために使う

Don't:

```tsx
function useMount(fn: () => void) {
  useEffect(() => {
    fn();
  }, []);
}

function ChatRoom({ roomId }: { roomId: string }) {
  useMount(() => {
    const connection = createConnection(roomId);
    connection.connect();
  });

  return <h1>Welcome to {roomId}</h1>;
}
```

Do:

```tsx
function useChatRoom(roomId: string) {
  useEffect(() => {
    const connection = createConnection(roomId);
    connection.connect();
    return () => connection.disconnect();
  }, [roomId]);
}

function ChatRoom({ roomId }: { roomId: string }) {
  useChatRoom(roomId);
  return <h1>Welcome to {roomId}</h1>;
}
```

Why:
- lifecycle 風 wrapper は依存関係を隠してしまう
- custom Hook は state を共有するのではなく、stateful logic を共有するために使う

References:
- https://react.dev/learn/reusing-logic-with-custom-hooks
- https://react.dev/learn/separating-events-from-effects
- https://overreacted.io/a-complete-guide-to-useeffect/

## 12. `useMemo` / `useCallback` / `memo` を最初から貼らない

Don't:

```tsx
function Counter() {
  const [count, setCount] = useState(0);

  const label = useMemo(() => `count: ${count}`, [count]);
  const handleClick = useCallback(() => setCount((c) => c + 1), []);

  return <button onClick={handleClick}>{label}</button>;
}
```

Do:

```tsx
function Counter() {
  const [count, setCount] = useState(0);

  return (
    <button onClick={() => setCount((c) => c + 1)}>
      count: {count}
    </button>
  );
}
```

必要になってから:

```tsx
function TodoList({ todos, filter }: Props) {
  const visibleTodos = useMemo(
    () => getFilteredTodos(todos, filter),
    [todos, filter]
  );

  return <SlowList items={visibleTodos} />;
}
```

Why:
- dependency surface が増えて読みづらくなる
- 根本原因が state 設計や `Effect` の乱用でも隠れてしまう

References:
- https://react.dev/learn/you-might-not-need-an-effect
- https://react.dev/reference/react/useMemo
- https://react.dev/reference/react/memo
- https://overreacted.io/before-you-memo/
- https://kentcdodds.com/blog/usememo-and-usecallback

## 13. React 外の mutable data を dummy state で再描画して合わせない

Don't:

```tsx
function Dashboard() {
  const counterRef = useRef({ value: 0 });
  const [refreshTick, setRefreshTick] = useState(0);

  function incrementCounter() {
    counterRef.current.value += 1;
    setRefreshTick((tick) => tick + 1);
  }

  return (
    <>
      <CounterPanel model={counterRef.current} onIncrement={incrementCounter} />
      <Summary counter={counterRef.current.value} />
    </>
  );
}

function CounterPanel({
  model,
  onIncrement,
}: {
  model: { value: number };
  onIncrement: () => void;
}) {
  return <button onClick={onIncrement}>count: {model.value}</button>;
}

function Summary({
  counter,
}: {
  counter: number;
}) {
  return <p>{counter}</p>;
}
```

Do:

```tsx
function Dashboard() {
  const [count, setCount] = useState(0);

  function handleIncrement() {
    setCount((current) => current + 1);
  }

  return (
    <>
      <CounterPanel count={count} onIncrement={handleIncrement} />
      <Summary count={count} />
    </>
  );
}

function CounterPanel({
  count,
  onIncrement,
}: {
  count: number;
  onIncrement: () => void;
}) {
  return <button onClick={onIncrement}>count: {count}</button>;
}

function Summary({ count }: { count: number }) {
  return <p>{count}</p>;
}
```

Why:
- data は owner から下へ流し、child は event handler を通じて intent を返す方が追いやすい
- dummy counter や `refreshTick` は原因ではなく症状で、根本問題は React 外の mutable data と ownership のズレ
- 「とにかく再描画させる」修復は、mutation と state ownership のミスを隠すだけ

References:
- https://react.dev/learn/thinking-in-react
- https://react.dev/learn/responding-to-events
- https://react.dev/learn/sharing-state-between-components
- https://react.dev/learn/referencing-values-with-refs
- https://react.dev/reference/rules/components-and-hooks-must-be-pure
- https://overreacted.io/writing-resilient-components/
- https://blog.isquaredsoftware.com/2020/05/blogged-answers-a-mostly-complete-guide-to-react-rendering-behavior/

## 14. public / reusable component API では raw setter より intent callback を prefer する

Don't:

```tsx
// Imagine this component is exported from a shared UI package.
export function CounterControl({
  count,
  setCount,
}: {
  count: number;
  setCount: React.Dispatch<React.SetStateAction<number>>;
}) {
  return (
    <button onClick={() => setCount(count + 1)}>
      count: {count}
    </button>
  );
}

function Parent() {
  const [count, setCount] = useState(0);
  return <CounterControl count={count} setCount={setCount} />;
}
```

Do:

```tsx
// Imagine this component is exported from a shared UI package.
export function CounterControl({
  count,
  onIncrement,
}: {
  count: number;
  onIncrement: () => void;
}) {
  return <button onClick={onIncrement}>count: {count}</button>;
}

function Parent() {
  const [count, setCount] = useState(0);

  function handleIncrement() {
    setCount((current) => current + 1);
  }

  return <CounterControl count={count} onIncrement={handleIncrement} />;
}
```

Why:
- raw setter を渡すと child が parent の state shape と更新方法を知りすぎる
- `onIncrement`, `onSelect`, `onSubmit` のような intent API の方が再利用しやすい
- 同一ファイル内の密結合な親子で setter を渡すこと自体を違反とは扱わない
- review では、public / reusable component API や tree をまたぐ境界で parent の内部 state 形が漏れていないかを見る

References:
- https://react.dev/learn/responding-to-events
- https://react.dev/learn/sharing-state-between-components
- https://overreacted.io/writing-resilient-components/

## 15. 中間 component を data tunnel にしない。まず composition を使う

Don't:

```tsx
function Page({
  user,
  posts,
  theme,
}: {
  user: User;
  posts: Post[];
  theme: Theme;
}) {
  return <Shell theme={theme} user={user} posts={posts} />;
}

function Shell({
  theme,
  user,
  posts,
}: {
  theme: Theme;
  user: User;
  posts: Post[];
}) {
  return (
    <div data-theme={theme}>
      <Content user={user} posts={posts} />
    </div>
  );
}

function Content({
  user,
  posts,
}: {
  user: User;
  posts: Post[];
}) {
  return (
    <>
      <Sidebar user={user} />
      <PostList posts={posts} />
    </>
  );
}
```

Do:

```tsx
function Page({
  user,
  posts,
  theme,
}: {
  user: User;
  posts: Post[];
  theme: Theme;
}) {
  return (
    <Shell theme={theme} sidebar={<Sidebar user={user} />}>
      <PostList posts={posts} />
    </Shell>
  );
}

function Shell({
  theme,
  sidebar,
  children,
}: {
  theme: Theme;
  sidebar: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div data-theme={theme}>
      {sidebar}
      {children}
    </div>
  );
}
```

Why:
- 中間 component がその data を使わないなら、props を運ぶだけの tunnel になっている
- composition や `children` を使うと tree の責務が明確になる
- context を増やす前に、component 抽出や `children` で穴を開けられないか確認する

References:
- https://react.dev/learn/passing-props-to-a-component
- https://react.dev/learn/passing-data-deeply-with-context
- https://overreacted.io/before-you-memo/
- https://blog.isquaredsoftware.com/2020/05/blogged-answers-a-mostly-complete-guide-to-react-rendering-behavior/

## 16. local state を必要以上に hoist しない

Don't:

```tsx
function Page() {
  const [query, setQuery] = useState("");

  return (
    <>
      <SearchBox value={query} onChange={setQuery} />
      <SlowSidebar />
      <SlowContent query={query} />
    </>
  );
}
```

Do:

```tsx
function Page() {
  return (
    <>
      <SearchSection />
      <SlowSidebar />
    </>
  );
}

function SearchSection() {
  const [query, setQuery] = useState("");

  return (
    <>
      <SearchBox value={query} onChange={setQuery} />
      <SlowContent query={query} />
    </>
  );
}
```

Why:
- local state は近い場所に置いた方が tree 全体の責務が小さくなる
- unrelated subtree まで毎回 rerender するなら、まず owner を下げるべき
- memoization で蓋をする前に、state を本当に必要な subtree に閉じ込める

References:
- https://react.dev/reference/react-dom/components/input#optimizing-re-rendering-on-every-keystroke
- https://react.dev/learn/sharing-state-between-components
- https://overreacted.io/before-you-memo/
- https://overreacted.io/writing-resilient-components/
- https://blog.isquaredsoftware.com/2020/05/blogged-answers-a-mostly-complete-guide-to-react-rendering-behavior/
