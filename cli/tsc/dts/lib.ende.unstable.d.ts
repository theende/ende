// Copyright 2018-2026 the Deno authors. MIT license.

/// <reference no-default-lib="true" />
/// <reference lib="ende.ns" />
/// <reference lib="esnext" />
/// <reference lib="es2022.intl" />

declare namespace Ende {
  export {}; // stop default export type behavior

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   *  Creates a presentable WebGPU surface from given window and
   *  display handles.
   *
   *  The parameters correspond to the table below:
   *
   *  | system            | winHandle     | displayHandle   |
   *  | ----------------- | ------------- | --------------- |
   *  | "cocoa" (macOS)   | -             | `NSView*`       |
   *  | "win32" (Windows) | `HWND`        | `HINSTANCE`     |
   *  | "x11" (Linux)     | Xlib `Window` | Xlib `Display*` |
   *  | "wayland" (Linux) | `wl_surface*` | `wl_display*`   |
   *
   * @category GPU
   * @experimental
   */
  export class UnsafeWindowSurface {
    /** The height of the window. */
    height: number;
    /** The width of the window. */
    width: number;

    constructor(
      options: {
        system: "cocoa" | "win32" | "x11" | "wayland";
        windowHandle: Ende.PointerValue<unknown>;
        displayHandle: Ende.PointerValue<unknown>;
        width: number;
        height: number;
      },
    );

    getContext(
      contextId: OffscreenRenderingContextId,
      options?: any,
    ): OffscreenRenderingContext | null;

    present(): void;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Represents membership of a IPv4 multicast group.
   *
   * @category Network
   * @experimental
   */
  export interface MulticastV4Membership {
    /** Leaves the multicast group. */
    leave: () => Promise<void>;
    /** Sets the multicast loopback option. If enabled, multicast packets will be looped back to the local socket. */
    setLoopback: (loopback: boolean) => Promise<void>;
    /** Sets the time-to-live of outgoing multicast packets for this socket. */
    setTTL: (ttl: number) => Promise<void>;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Represents membership of a IPv6 multicast group.
   *
   * @category Network
   * @experimental
   */
  export interface MulticastV6Membership {
    /** Leaves the multicast group. */
    leave: () => Promise<void>;
    /** Sets the multicast loopback option. If enabled, multicast packets will be looped back to the local socket. */
    setLoopback: (loopback: boolean) => Promise<void>;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * A generic transport listener for message-oriented protocols.
   *
   * @category Network
   * @experimental
   */
  export interface DatagramConn
    extends AsyncIterable<[Uint8Array<ArrayBuffer>, Addr]> {
    /** Joins an IPv4 multicast group. */
    joinMulticastV4(
      address: string,
      networkInterface: string,
    ): Promise<MulticastV4Membership>;

    /** Joins an IPv6 multicast group. */
    joinMulticastV6(
      address: string,
      networkInterface: number,
    ): Promise<MulticastV6Membership>;

    /** Waits for and resolves to the next message to the instance.
     *
     * Messages are received in the format of a tuple containing the data array
     * and the address information.
     */
    receive(p?: Uint8Array): Promise<[Uint8Array<ArrayBuffer>, Addr]>;
    /** Sends a message to the target via the connection. The method resolves
     * with the number of bytes sent. */
    send(p: Uint8Array, addr: Addr): Promise<number>;
    /** Close closes the socket. Any pending message promises will be rejected
     * with errors. */
    close(): void;
    /** Return the address of the instance. */
    readonly addr: Addr;
    [Symbol.asyncIterator](): AsyncIterableIterator<
      [Uint8Array<ArrayBuffer>, Addr]
    >;
  }

  /**
   * @category Network
   * @experimental
   */
  export interface TcpListenOptions extends ListenOptions {
    /** When `true` the SO_REUSEPORT flag will be set on the listener. This
     * allows multiple processes to listen on the same address and port.
     *
     * On Linux this will cause the kernel to distribute incoming connections
     * across the different processes that are listening on the same address and
     * port.
     *
     * This flag is only supported on Linux. It is silently ignored on other
     * platforms.
     *
     * @default {false} */
    reusePort?: boolean;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Unstable options which can be set when opening a datagram listener via
   * {@linkcode Ende.listenDatagram}.
   *
   * @category Network
   * @experimental
   */
  export interface UdpListenOptions extends ListenOptions {
    /** When `true` the specified address will be reused, even if another
     * process has already bound a socket on it. This effectively steals the
     * socket from the listener.
     *
     * @default {false} */
    reuseAddress?: boolean;

    /** When `true`, sent multicast packets will be looped back to the local socket.
     *
     * @default {false} */
    loopback?: boolean;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Listen announces on the local transport address.
   *
   * ```ts
   * const listener1 = Ende.listenDatagram({
   *   port: 80,
   *   transport: "udp"
   * });
   * const listener2 = Ende.listenDatagram({
   *   hostname: "golang.org",
   *   port: 80,
   *   transport: "udp"
   * });
   * ```
   *
   * Requires `allow-net` permission.
   *
   * @tags allow-net
   * @category Network
   * @experimental
   */
  export function listenDatagram(
    options: UdpListenOptions & { transport: "udp" },
  ): DatagramConn;

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Unstable options which can be set when opening a `unixpacket` datagram
   * listener via {@linkcode Ende.listenDatagram}.
   *
   * @category Network
   * @experimental
   */
  export interface UnixListenDatagramOptions {
    /** A path to the Unix Socket. When omitted the socket is left unbound, so
     * it can be used to send messages but cannot receive them. */
    path?: string;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Listen announces on the local transport address.
   *
   * ```ts
   * const listener = Ende.listenDatagram({
   *   path: "/foo/bar.sock",
   *   transport: "unixpacket"
   * });
   * ```
   *
   * Requires `allow-read`, `allow-write` and `allow-net` permission. The
   * `allow-net` grant may be scoped to the socket path with
   * `--allow-net=unix:<absolute-path>`.
   *
   * @tags allow-read, allow-write, allow-net
   * @category Network
   * @experimental
   */
  export function listenDatagram(
    options: UnixListenDatagramOptions & { transport: "unixpacket" },
  ): DatagramConn;

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * CronScheduleExpression is used as the type of `minute`, `hour`,
   * `dayOfMonth`, `month`, and `dayOfWeek` in {@linkcode CronSchedule}.
   * @category Cloud
   * @experimental
   */
  export type CronScheduleExpression = number | { exact: number | number[] } | {
    start?: number;
    end?: number;
    every?: number;
  };

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * CronSchedule is the interface used for JSON format
   * cron `schedule`.
   * @category Cloud
   * @experimental
   */
  export interface CronSchedule {
    minute?: CronScheduleExpression;
    hour?: CronScheduleExpression;
    dayOfMonth?: CronScheduleExpression;
    month?: CronScheduleExpression;
    dayOfWeek?: CronScheduleExpression;
  }

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Create a cron job that will periodically execute the provided handler
   * callback based on the specified schedule.
   *
   * This is an unstable API and requires the `--unstable-cron` flag to be
   * passed when running Ende.
   *
   * ```ts
   * Ende.cron("sample cron", "20 * * * *", () => {
   *   console.log("cron job executed");
   * });
   * ```
   *
   * ```ts
   * Ende.cron("sample cron", { hour: { every: 6 } }, () => {
   *   console.log("cron job executed");
   * });
   * ```
   *
   * `schedule` can be a string in the Unix cron format or in JSON format
   * as specified by interface {@linkcode CronSchedule}, where time is specified
   * using UTC time zone.
   *
   * Calling `Ende.cron` registers the job and immediately returns. The returned
   * promise should not be awaited to wait for the job to run, it is provided
   * only to surface registration errors. The job keeps running in the
   * background for the lifetime of the process. To be able to stop a cron job
   * (for example during a graceful shutdown), use the overload that accepts an
   * `options` object with an {@linkcode AbortSignal}.
   *
   * @category Cloud
   * @experimental
   */
  export function cron(
    name: string,
    schedule: string | CronSchedule,
    handler: () => Promise<void> | void,
  ): Promise<void>;

  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Create a cron job that will periodically execute the provided handler
   * callback based on the specified schedule.
   *
   * ```ts
   * Ende.cron("sample cron", "20 * * * *", {
   *   backoffSchedule: [10, 20]
   * }, () => {
   *   console.log("cron job executed");
   * });
   * ```
   *
   * `schedule` can be a string in the Unix cron format or in JSON format
   * as specified by interface {@linkcode CronSchedule}, where time is specified
   * using UTC time zone.
   *
   * `backoffSchedule` option can be used to specify the retry policy for failed
   * executions. Each element in the array represents the number of milliseconds
   * to wait before retrying the execution. For example, `[1000, 5000, 10000]`
   * means that a failed execution will be retried at most 3 times, with 1
   * second, 5 seconds, and 10 seconds delay between each retry. There is a
   * limit of 5 retries and a maximum interval of 1 hour (3600000 milliseconds).
   *
   * `signal` option can be used to stop the cron job by passing an
   * {@linkcode AbortSignal} and aborting it. This is useful for implementing a
   * graceful shutdown, since aborting the signal unregisters the job:
   *
   * ```ts
   * const ac = new AbortController();
   *
   * Ende.cron("sample cron", "20 * * * *", { signal: ac.signal }, () => {
   *   console.log("cron job executed");
   * });
   *
   * // Later, stop the cron job from running again.
   * ac.abort();
   * ```
   *
   * @category Cloud
   * @experimental
   */
  export function cron(
    name: string,
    schedule: string | CronSchedule,
    options: { backoffSchedule?: number[]; signal?: AbortSignal },
    handler: () => Promise<void> | void,
  ): Promise<void>;

  /**
   * @category Linter
   * @experimental
   */
  export namespace lint {
    /**
     * @category Linter
     * @experimental
     */
    export type Range = [number, number];

    /**
     * @category Linter
     * @experimental
     */
    export interface Fix {
      range: Range;
      text?: string;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface Fixer {
      insertTextAfter(node: Node, text: string): Fix;
      insertTextAfterRange(range: Range, text: string): Fix;
      insertTextBefore(node: Node, text: string): Fix;
      insertTextBeforeRange(range: Range, text: string): Fix;
      remove(node: Node): Fix;
      removeRange(range: Range): Fix;
      replaceText(node: Node, text: string): Fix;
      replaceTextRange(range: Range, text: string): Fix;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ReportData {
      node?: Node;
      range?: Range;
      message: string;
      hint?: string;
      fix?(fixer: Fixer): Fix | Iterable<Fix>;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface SourceCode {
      /**
       * Get the source test of a node. Omit `node` to get the
       * full source code.
       */
      getText(node?: Node): string;
      /**
       * Returns array of ancestors of the current node, excluding the
       * current node.
       */
      getAncestors(node: Node): Node[];

      /**
       * Get all comments inside the source.
       */
      getAllComments(): Array<LineComment | BlockComment>;

      /**
       * Get leading comments before a node.
       */
      getCommentsBefore(node: Node): Array<LineComment | BlockComment>;

      /**
       * Get trailing comments after a node.
       */
      getCommentsAfter(node: Node): Array<LineComment | BlockComment>;

      /**
       * Get comments inside a node.
       */
      getCommentsInside(node: Node): Array<LineComment | BlockComment>;

      /**
       * Get the full source code.
       */
      text: string;
      /**
       * Get the root node of the file. It's always the `Program` node.
       */
      ast: Program;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface RuleContext {
      /**
       * The running rule id: `<plugin-name>/<rule-name>`
       */
      id: string;
      /**
       * Name of the file that's currently being linted.
       */
      filename: string;
      /**
       * Helper methods for working with the raw source code.
       */
      sourceCode: SourceCode;
      /**
       * Report a lint error.
       */
      report(data: ReportData): void;
      /**
       * @deprecated Use `ctx.filename` instead.
       */
      getFilename(): string;
      /**
       * @deprecated Use `ctx.sourceCode` instead.
       */
      getSourceCode(): SourceCode;
    }

    /**
     * @category Linter
     * @experimental
     */
    export type LintVisitor =
      & {
        [P in Node["type"]]?: (node: Extract<Node, { type: P }>) => void;
      }
      & {
        [P in Node["type"] as `${P}:exit`]?: (
          node: Extract<Node, { type: P }>,
        ) => void;
      }
      & // Custom selectors which cannot be typed by us
      // deno-lint-ignore no-explicit-any
      Partial<{ [key: string]: (node: any) => void }>;

    /**
     * @category Linter
     * @experimental
     */
    export interface Rule {
      create(ctx: RuleContext): LintVisitor;
      destroy?(ctx: RuleContext): void;
    }

    /**
     * In your plugins file do something like
     *
     * ```ts
     * export default {
     *   name: "my-plugin",
     *   rules: {
     *     "no-foo": {
     *        create(ctx) {
     *          return {
     *             VariableDeclaration(node) {}
     *          }
     *        }
     *     }
     *   }
     * } satisfies Ende.lint.Plugin
     * ```
     * @category Linter
     * @experimental
     */
    export interface Plugin {
      name: string;
      rules: Record<string, Rule>;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface Diagnostic {
      id: string;
      message: string;
      hint?: string;
      range: Range;
      fix?: Fix[];
    }

    /**
     * This API is useful for testing lint plugins.
     *
     * It throws an error if it's not used in `ende test` subcommand.
     * @category Linter
     * @experimental
     */
    export function runPlugin(
      plugin: Plugin,
      fileName: string,
      source: string,
    ): Diagnostic[];

    /**
     * @category Linter
     * @experimental
     */
    export interface Program {
      type: "Program";
      range: Range;
      sourceType: "module" | "script";
      body: Statement[];
      comments: Array<LineComment | BlockComment>;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ImportSpecifier {
      type: "ImportSpecifier";
      range: Range;
      imported: Identifier | StringLiteral;
      local: Identifier;
      importKind: "type" | "value";
      parent: ExportAllDeclaration | ExportNamedDeclaration | ImportDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ImportDefaultSpecifier {
      type: "ImportDefaultSpecifier";
      range: Range;
      local: Identifier;
      parent: ImportDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ImportNamespaceSpecifier {
      type: "ImportNamespaceSpecifier";
      range: Range;
      local: Identifier;
      parent: ImportDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ImportAttribute {
      type: "ImportAttribute";
      range: Range;
      key: Identifier | Literal;
      value: Literal;
      parent:
        | ExportAllDeclaration
        | ExportNamedDeclaration
        | ImportDeclaration
        | TSImportType;
    }

    /**
     * An import declaration, examples:
     * @category Linter
     * @experimental
     */
    export interface ImportDeclaration {
      type: "ImportDeclaration";
      range: Range;
      importKind: "type" | "value";
      source: StringLiteral;
      specifiers: Array<
        | ImportDefaultSpecifier
        | ImportNamespaceSpecifier
        | ImportSpecifier
      >;
      attributes: ImportAttribute[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ExportDefaultDeclaration {
      type: "ExportDefaultDeclaration";
      range: Range;
      declaration:
        | ClassDeclaration
        | Expression
        | FunctionDeclaration
        | TSDeclareFunction
        | TSEnumDeclaration
        | TSInterfaceDeclaration
        | TSModuleDeclaration
        | TSTypeAliasDeclaration
        | VariableDeclaration;
      exportKind: "type" | "value";
      parent: BlockStatement | Program | TSModuleBlock;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ExportNamedDeclaration {
      type: "ExportNamedDeclaration";
      range: Range;
      exportKind: "type" | "value";
      specifiers: ExportSpecifier[];
      declaration:
        | ClassDeclaration
        | FunctionDeclaration
        | TSDeclareFunction
        | TSEnumDeclaration
        | TSImportEqualsDeclaration
        | TSInterfaceDeclaration
        | TSModuleDeclaration
        | TSTypeAliasDeclaration
        | VariableDeclaration
        | null;
      source: StringLiteral | null;
      attributes: ImportAttribute[];
      parent: BlockStatement | Program | TSModuleBlock;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ExportAllDeclaration {
      type: "ExportAllDeclaration";
      range: Range;
      exportKind: "type" | "value";
      exported: Identifier | null;
      source: StringLiteral;
      attributes: ImportAttribute[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNamespaceExportDeclaration {
      type: "TSNamespaceExportDeclaration";
      range: Range;
      id: Identifier;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSImportEqualsDeclaration {
      type: "TSImportEqualsDeclaration";
      range: Range;
      importKind: "type" | "value";
      id: Identifier;
      moduleReference: Identifier | TSExternalModuleReference | TSQualifiedName;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSExternalModuleReference {
      type: "TSExternalModuleReference";
      range: Range;
      expression: StringLiteral;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface ExportSpecifier {
      type: "ExportSpecifier";
      range: Range;
      exportKind: "type" | "value";
      exported: Identifier | StringLiteral;
      local: Identifier | StringLiteral;
      parent: ExportNamedDeclaration;
    }

    /**
     * Variable declaration.
     * @category Linter
     * @experimental
     */
    export interface VariableDeclaration {
      type: "VariableDeclaration";
      range: Range;
      declare: boolean;
      kind: "let" | "var" | "const" | "await using" | "using";
      declarations: VariableDeclarator[];
      parent: Node;
    }

    /**
     * A VariableDeclaration can declare multiple variables. This node
     * represents a single declaration out of that.
     * @category Linter
     * @experimental
     */
    export interface VariableDeclarator {
      type: "VariableDeclarator";
      range: Range;
      id: ArrayPattern | ObjectPattern | Identifier;
      init: Expression | null;
      definite: boolean;
      parent: VariableDeclaration;
    }

    /**
     * Function/Method parameter
     * @category Linter
     * @experimental
     */
    export type Parameter =
      | ArrayPattern
      | AssignmentPattern
      | Identifier
      | ObjectPattern
      | RestElement
      | TSParameterProperty;

    /**
     * TypeScript accessibility modifiers used in classes
     * @category Linter
     * @experimental
     */
    export type Accessibility = "private" | "protected" | "public";

    /**
     * Declares a function in the current scope
     * @category Linter
     * @experimental
     */
    export interface FunctionDeclaration {
      type: "FunctionDeclaration";
      range: Range;
      declare: boolean;
      async: boolean;
      generator: boolean;
      id: Identifier | null;
      typeParameters: TSTypeParameterDeclaration | undefined;
      returnType: TSTypeAnnotation | undefined;
      body: BlockStatement | null;
      params: Parameter[];
      parent:
        | BlockStatement
        | ExportDefaultDeclaration
        | ExportNamedDeclaration
        | Program;
    }

    /**
     * Experimental: Decorators
     * @category Linter
     * @experimental
     */
    export interface Decorator {
      type: "Decorator";
      range: Range;
      expression:
        | ArrayExpression
        | ArrayPattern
        | ArrowFunctionExpression
        | CallExpression
        | ClassExpression
        | FunctionExpression
        | Identifier
        | JSXElement
        | JSXFragment
        | Literal
        | TemplateLiteral
        | MemberExpression
        | MetaProperty
        | ObjectExpression
        | ObjectPattern
        | SequenceExpression
        | Super
        | TaggedTemplateExpression
        | ThisExpression
        | TSAsExpression
        | TSNonNullExpression
        | TSTypeAssertion;
      parent: Node;
    }

    /**
     * Declares a class in the current scope
     * @category Linter
     * @experimental
     */
    export interface ClassDeclaration {
      type: "ClassDeclaration";
      range: Range;
      declare: boolean;
      abstract: boolean;
      id: Identifier | null;
      superClass:
        | ArrayExpression
        | ArrayPattern
        | ArrowFunctionExpression
        | CallExpression
        | ClassExpression
        | FunctionExpression
        | Identifier
        | JSXElement
        | JSXFragment
        | Literal
        | TemplateLiteral
        | MemberExpression
        | MetaProperty
        | ObjectExpression
        | ObjectPattern
        | SequenceExpression
        | Super
        | TaggedTemplateExpression
        | ThisExpression
        | TSAsExpression
        | TSNonNullExpression
        | TSTypeAssertion
        | null;
      implements: TSClassImplements[];
      body: ClassBody;
      parent: Node;
    }

    /**
     * Similar to ClassDeclaration but for declaring a class as an
     * expression. The main difference is that the class name(=id) can
     * be omitted.
     * @category Linter
     * @experimental
     */
    export interface ClassExpression {
      type: "ClassExpression";
      range: Range;
      declare: boolean;
      abstract: boolean;
      id: Identifier | null;
      superClass:
        | ArrayExpression
        | ArrayPattern
        | ArrowFunctionExpression
        | CallExpression
        | ClassExpression
        | FunctionExpression
        | Identifier
        | JSXElement
        | JSXFragment
        | Literal
        | TemplateLiteral
        | MemberExpression
        | MetaProperty
        | ObjectExpression
        | ObjectPattern
        | SequenceExpression
        | Super
        | TaggedTemplateExpression
        | ThisExpression
        | TSAsExpression
        | TSNonNullExpression
        | TSTypeAssertion
        | null;
      superTypeArguments: TSTypeParameterInstantiation | undefined;
      typeParameters: TSTypeParameterDeclaration | undefined;
      implements: TSClassImplements[];
      body: ClassBody;
      parent: Node;
    }

    /**
     * Represents the body of a class and contains all members
     * @category Linter
     * @experimental
     */
    export interface ClassBody {
      type: "ClassBody";
      range: Range;
      body: Array<
        | AccessorProperty
        | MethodDefinition
        | PropertyDefinition
        | StaticBlock
        // Stage 1 Proposal:
        // https://github.com/tc39/proposal-grouped-and-auto-accessors
        // | TSAbstractAccessorProperty
        | TSAbstractMethodDefinition
        | TSAbstractPropertyDefinition
        | TSIndexSignature
      >;
      parent: ClassDeclaration | ClassExpression;
    }

    /**
     * Static class initializiation block.
     * @category Linter
     * @experimental
     */
    export interface StaticBlock {
      type: "StaticBlock";
      range: Range;
      body: Statement[];
      parent: ClassBody;
    }

    // Stage 1 Proposal:
    // https://github.com/tc39/proposal-grouped-and-auto-accessors
    // | TSAbstractAccessorProperty
    /**
     * @category Linter
     * @experimental
     */
    export interface AccessorProperty {
      type: "AccessorProperty";
      range: Range;
      declare: boolean;
      computed: boolean;
      optional: boolean;
      override: boolean;
      readonly: boolean;
      static: boolean;
      accessibility: Accessibility | undefined;
      decorators: Decorator[];
      key: Expression | Identifier | NumberLiteral | StringLiteral;
      value: Expression | null;
      parent: ClassBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface PropertyDefinition {
      type: "PropertyDefinition";
      range: Range;
      declare: boolean;
      computed: boolean;
      optional: boolean;
      override: boolean;
      readonly: boolean;
      static: boolean;
      accessibility: Accessibility | undefined;
      decorators: Decorator[];
      key:
        | Expression
        | Identifier
        | NumberLiteral
        | StringLiteral
        | PrivateIdentifier;
      value: Expression | null;
      typeAnnotation: TSTypeAnnotation | undefined;
      parent: ClassBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface MethodDefinition {
      type: "MethodDefinition";
      range: Range;
      declare: boolean;
      computed: boolean;
      optional: boolean;
      override: boolean;
      readonly: boolean;
      static: boolean;
      kind: "constructor" | "get" | "method" | "set";
      accessibility: Accessibility | undefined;
      decorators: Decorator[];
      key:
        | PrivateIdentifier
        | Identifier
        | NumberLiteral
        | StringLiteral
        | Expression;
      value: FunctionExpression | TSEmptyBodyFunctionExpression;
      parent: ClassBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface BlockStatement {
      type: "BlockStatement";
      range: Range;
      body: Statement[];
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * The `debugger;` statement.
     * @category Linter
     * @experimental
     */
    export interface DebuggerStatement {
      type: "DebuggerStatement";
      range: Range;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Legacy JavaScript feature, that's discouraged from being used today.
     * @deprecated
     * @category Linter
     * @experimental
     */
    export interface WithStatement {
      type: "WithStatement";
      range: Range;
      object: Expression;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Returns a value from a function.
     * @category Linter
     * @experimental
     */
    export interface ReturnStatement {
      type: "ReturnStatement";
      range: Range;
      argument: Expression | null;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Custom control flow based on labels.
     * @category Linter
     * @experimental
     */
    export interface LabeledStatement {
      type: "LabeledStatement";
      range: Range;
      label: Identifier;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Break any loop or labeled statement, example:
     *
     * ```ts
     * while (true) {
     *   break;
     * }
     *
     * for (let i = 0; i < 10; i++) {
     *   if (i > 5) break;
     * }
     * ```
     * @category Linter
     * @experimental
     */
    export interface BreakStatement {
      type: "BreakStatement";
      range: Range;
      label: Identifier | null;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Terminates the current loop and continues with the next iteration.
     * @category Linter
     * @experimental
     */
    export interface ContinueStatement {
      type: "ContinueStatement";
      range: Range;
      label: Identifier | null;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Execute a statement the test passes, otherwise the alternate
     * statement, if it was defined.
     * @category Linter
     * @experimental
     */
    export interface IfStatement {
      type: "IfStatement";
      range: Range;
      test: Expression;
      consequent: Statement;
      alternate: Statement | null;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Match an expression against a series of cases.
     * @category Linter
     * @experimental
     */
    export interface SwitchStatement {
      type: "SwitchStatement";
      range: Range;
      discriminant: Expression;
      cases: SwitchCase[];
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * A single case of a SwitchStatement.
     * @category Linter
     * @experimental
     */
    export interface SwitchCase {
      type: "SwitchCase";
      range: Range;
      test: Expression | null;
      consequent: Statement[];
      parent: SwitchStatement;
    }

    /**
     * Throw a user defined exception. Stops execution
     * of the current function.
     * @category Linter
     * @experimental
     */
    export interface ThrowStatement {
      type: "ThrowStatement";
      range: Range;
      argument: Expression;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Run a loop while the test expression is truthy.
     * @category Linter
     * @experimental
     */
    export interface WhileStatement {
      type: "WhileStatement";
      range: Range;
      test: Expression;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Re-run loop for as long as test expression is truthy.
     * @category Linter
     * @experimental
     */
    export interface DoWhileStatement {
      type: "DoWhileStatement";
      range: Range;
      test: Expression;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Classic for-loop.
     * @category Linter
     * @experimental
     */
    export interface ForStatement {
      type: "ForStatement";
      range: Range;
      init: Expression | VariableDeclaration | null;
      test: Expression | null;
      update: Expression | null;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Enumerate over all enumerable string properties of an object.
     * @category Linter
     * @experimental
     */
    export interface ForInStatement {
      type: "ForInStatement";
      range: Range;
      left: Expression | VariableDeclaration;
      right: Expression;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Iterate over sequence of values from an iterator.
     * @category Linter
     * @experimental
     */
    export interface ForOfStatement {
      type: "ForOfStatement";
      range: Range;
      await: boolean;
      left: Expression | VariableDeclaration;
      right: Expression;
      body: Statement;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Statement that holds an expression.
     * @category Linter
     * @experimental
     */
    export interface ExpressionStatement {
      type: "ExpressionStatement";
      range: Range;
      expression: Expression;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Try/catch statement
     * @category Linter
     * @experimental
     */
    export interface TryStatement {
      type: "TryStatement";
      range: Range;
      block: BlockStatement;
      handler: CatchClause | null;
      finalizer: BlockStatement | null;
      parent:
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * The catch clause of a try/catch statement
     * @category Linter
     * @experimental
     */
    export interface CatchClause {
      type: "CatchClause";
      range: Range;
      param: ArrayPattern | ObjectPattern | Identifier | null;
      body: BlockStatement;
      parent: TryStatement;
    }

    /**
     * An array literal
     * @category Linter
     * @experimental
     */
    export interface ArrayExpression {
      type: "ArrayExpression";
      range: Range;
      elements: Array<Expression | SpreadElement>;
      parent: Node;
    }

    /**
     * An object literal.
     * @category Linter
     * @experimental
     */
    export interface ObjectExpression {
      type: "ObjectExpression";
      range: Range;
      properties: Array<Property | SpreadElement>;
      parent: Node;
    }

    /**
     * Compare left and right value with the specifier operator.
     * @category Linter
     * @experimental
     */
    export interface BinaryExpression {
      type: "BinaryExpression";
      range: Range;
      operator:
        | "&"
        | "**"
        | "*"
        | "||"
        | "|"
        | "^"
        | "==="
        | "=="
        | "!=="
        | "!="
        | ">="
        | ">>>"
        | ">>"
        | ">"
        | "in"
        | "instanceof"
        | "<="
        | "<<"
        | "<"
        | "-"
        | "%"
        | "+"
        | "/";
      left: Expression | PrivateIdentifier;
      right: Expression;
      parent: Node;
    }

    /**
     * Chain expressions based on the operator specified
     * @category Linter
     * @experimental
     */
    export interface LogicalExpression {
      type: "LogicalExpression";
      range: Range;
      operator: "&&" | "??" | "||";
      left: Expression;
      right: Expression;
      parent: Node;
    }

    /**
     * Declare a function as an expression. Similar to `FunctionDeclaration`,
     * with an optional name (=id).
     * @category Linter
     * @experimental
     */
    export interface FunctionExpression {
      type: "FunctionExpression";
      range: Range;
      async: boolean;
      generator: boolean;
      id: Identifier | null;
      typeParameters: TSTypeParameterDeclaration | undefined;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      body: BlockStatement;
      parent: Node;
    }

    /**
     * Arrow function expression
     * @category Linter
     * @experimental
     */
    export interface ArrowFunctionExpression {
      type: "ArrowFunctionExpression";
      range: Range;
      async: boolean;
      generator: boolean;
      id: null;
      typeParameters: TSTypeParameterDeclaration | undefined;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      body: BlockStatement | Expression;
      parent: Node;
    }

    /**
     * The `this` keyword used in classes.
     * @category Linter
     * @experimental
     */
    export interface ThisExpression {
      type: "ThisExpression";
      range: Range;
      parent: Node;
    }

    /**
     * The `super` keyword used in classes.
     * @category Linter
     * @experimental
     */
    export interface Super {
      type: "Super";
      range: Range;
      parent: Node;
    }

    /**
     * Apply operand on value based on the specified operator.
     * @category Linter
     * @experimental
     */
    export interface UnaryExpression {
      type: "UnaryExpression";
      range: Range;
      operator: "!" | "+" | "~" | "-" | "delete" | "typeof" | "void";
      argument: Expression;
      parent: Node;
    }

    /**
     * Create a new instance of a class.
     * @category Linter
     * @experimental
     */
    export interface NewExpression {
      type: "NewExpression";
      range: Range;
      callee: Expression;
      typeArguments: TSTypeParameterInstantiation | undefined;
      arguments: Array<Expression | SpreadElement>;
      parent: Node;
    }

    /**
     * Dynamically import a module.
     * @category Linter
     * @experimental
     */
    export interface ImportExpression {
      type: "ImportExpression";
      range: Range;
      source: Expression;
      options: Expression | null;
      parent: Node;
    }

    /**
     * A function call.
     * @category Linter
     * @experimental
     */
    export interface CallExpression {
      type: "CallExpression";
      range: Range;
      optional: boolean;
      callee: Expression;
      typeArguments: TSTypeParameterInstantiation | null;
      arguments: Array<Expression | SpreadElement>;
      parent: Node;
    }

    /**
     * Syntactic sugar to increment or decrement a value.
     * @category Linter
     * @experimental
     */
    export interface UpdateExpression {
      type: "UpdateExpression";
      range: Range;
      prefix: boolean;
      operator: "++" | "--";
      argument: Expression;
      parent: Node;
    }

    /**
     * Updaate a variable or property.
     * @category Linter
     * @experimental
     */
    export interface AssignmentExpression {
      type: "AssignmentExpression";
      range: Range;
      operator:
        | "&&="
        | "&="
        | "**="
        | "*="
        | "||="
        | "|="
        | "^="
        | "="
        | ">>="
        | ">>>="
        | "<<="
        | "-="
        | "%="
        | "+="
        | "??="
        | "/=";
      left: Expression;
      right: Expression;
      parent: Node;
    }

    /**
     * Inline if-statement.
     * @category Linter
     * @experimental
     */
    export interface ConditionalExpression {
      type: "ConditionalExpression";
      range: Range;
      test: Expression;
      consequent: Expression;
      alternate: Expression;
      parent: Node;
    }

    /**
     * MemberExpression
     * @category Linter
     * @experimental
     */
    export interface MemberExpression {
      type: "MemberExpression";
      range: Range;
      optional: boolean;
      computed: boolean;
      object: Expression;
      property: Expression | Identifier | PrivateIdentifier;
      parent: Node;
    }

    /**
     * ChainExpression
     * @category Linter
     * @experimental
     */
    export interface ChainExpression {
      type: "ChainExpression";
      range: Range;
      expression:
        | CallExpression
        | MemberExpression
        | TSNonNullExpression;
      parent: Node;
    }

    /**
     * Execute multiple expressions in sequence.
     * @category Linter
     * @experimental
     */
    export interface SequenceExpression {
      type: "SequenceExpression";
      range: Range;
      expressions: Expression[];
      parent: Node;
    }

    /**
     * A template literal string.
     * @category Linter
     * @experimental
     */
    export interface TemplateLiteral {
      type: "TemplateLiteral";
      range: Range;
      quasis: TemplateElement[];
      expressions: Expression[];
      parent: Node;
    }

    /**
     * The static portion of a template literal.
     * @category Linter
     * @experimental
     */
    export interface TemplateElement {
      type: "TemplateElement";
      range: Range;
      tail: boolean;
      raw: string;
      cooked: string;
      parent: TemplateLiteral | TSTemplateLiteralType;
    }

    /**
     * Tagged template expression.
     * @category Linter
     * @experimental
     */
    export interface TaggedTemplateExpression {
      type: "TaggedTemplateExpression";
      range: Range;
      tag: Expression;
      typeArguments: TSTypeParameterInstantiation | undefined;
      quasi: TemplateLiteral;
      parent: Node;
    }

    /**
     * Pause or resume a generator function.
     * @category Linter
     * @experimental
     */
    export interface YieldExpression {
      type: "YieldExpression";
      range: Range;
      delegate: boolean;
      argument: Expression | null;
      parent: Node;
    }

    /**
     * Await a `Promise` and get its fulfilled value.
     * @category Linter
     * @experimental
     */
    export interface AwaitExpression {
      type: "AwaitExpression";
      range: Range;
      argument: Expression;
      parent: Node;
    }

    /**
     * Can either be `import.meta` or `new.target`.
     * @category Linter
     * @experimental
     */
    export interface MetaProperty {
      type: "MetaProperty";
      range: Range;
      meta: Identifier;
      property: Identifier;
      parent: Node;
    }

    /**
     * Custom named node by the developer. Can be a variable name,
     * a function name, parameter, etc.
     * @category Linter
     * @experimental
     */
    export interface Identifier {
      type: "Identifier";
      range: Range;
      name: string;
      optional: boolean;
      typeAnnotation: TSTypeAnnotation | undefined;
      parent: Node;
    }

    /**
     * Private members inside of classes, must start with `#`.
     * @category Linter
     * @experimental
     */
    export interface PrivateIdentifier {
      type: "PrivateIdentifier";
      range: Range;
      name: string;
      parent:
        | TSAbstractPropertyDefinition
        | TSPropertySignature
        | PropertyDefinition
        | MethodDefinition
        | BinaryExpression
        | MemberExpression;
    }

    /**
     * Assign default values in parameters.
     * @category Linter
     * @experimental
     */
    export interface AssignmentPattern {
      type: "AssignmentPattern";
      range: Range;
      left: ArrayPattern | ObjectPattern | Identifier;
      right: Expression;
      parent: Node;
    }

    /**
     * Destructure an array.
     * @category Linter
     * @experimental
     */
    export interface ArrayPattern {
      type: "ArrayPattern";
      range: Range;
      optional: boolean;
      typeAnnotation: TSTypeAnnotation | undefined;
      elements: Array<
        | ArrayPattern
        | AssignmentPattern
        | Identifier
        | MemberExpression
        | ObjectPattern
        | RestElement
        | null
      >;
      parent: Node;
    }

    /**
     * Destructure an object.
     * @category Linter
     * @experimental
     */
    export interface ObjectPattern {
      type: "ObjectPattern";
      range: Range;
      optional: boolean;
      typeAnnotation: TSTypeAnnotation | undefined;
      properties: Array<Property | RestElement>;
      parent: Node;
    }

    /**
     * The rest of function parameters.
     * @category Linter
     * @experimental
     */
    export interface RestElement {
      type: "RestElement";
      range: Range;
      typeAnnotation: TSTypeAnnotation | undefined;
      argument:
        | ArrayPattern
        | AssignmentPattern
        | Identifier
        | MemberExpression
        | ObjectPattern
        | RestElement;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface SpreadElement {
      type: "SpreadElement";
      range: Range;
      argument: Expression;
      parent:
        | ArrayExpression
        | CallExpression
        | NewExpression
        | ObjectExpression;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface Property {
      type: "Property";
      range: Range;
      shorthand: boolean;
      computed: boolean;
      method: boolean;
      kind: "get" | "init" | "set";
      key: Expression | Identifier | NumberLiteral | StringLiteral;
      value:
        | AssignmentPattern
        | ArrayPattern
        | ObjectPattern
        | Identifier
        | Expression
        | TSEmptyBodyFunctionExpression;
      parent: ObjectExpression | ObjectPattern;
    }

    /**
     * Represents numbers that are too high or too low to be represented
     * by the `number` type.
     *
     * ```ts
     * const a = 9007199254740991n;
     * ```
     * @category Linter
     * @experimental
     */
    export interface BigIntLiteral {
      type: "Literal";
      range: Range;
      raw: string;
      bigint: string;
      value: bigint;
      parent: Node;
    }

    /**
     * Either `true` or `false`
     * @category Linter
     * @experimental
     */
    export interface BooleanLiteral {
      type: "Literal";
      range: Range;
      raw: "false" | "true";
      value: boolean;
      parent: Node;
    }

    /**
     * A number literal
     *
     * ```ts
     * 1;
     * 1.2;
     * ```
     * @category Linter
     * @experimental
     */
    export interface NumberLiteral {
      type: "Literal";
      range: Range;
      raw: string;
      value: number;
      parent: Node;
    }

    /**
     * The `null` literal
     * @category Linter
     * @experimental
     */
    export interface NullLiteral {
      type: "Literal";
      range: Range;
      raw: "null";
      value: null;
      parent: Node;
    }

    /**
     * A string literal
     *
     * ```ts
     * "foo";
     * 'foo "bar"';
     * ```
     * @category Linter
     * @experimental
     */
    export interface StringLiteral {
      type: "Literal";
      range: Range;
      raw: string;
      value: string;
      parent: Node;
    }

    /**
     * A regex literal:
     *
     * ```ts
     * /foo(bar|baz)$/g
     * ```
     * @category Linter
     * @experimental
     */
    export interface RegExpLiteral {
      type: "Literal";
      range: Range;
      raw: string;
      regex: {
        flags: string;
        pattern: string;
      };
      value: RegExp | null;
      parent: Node;
    }

    /**
     * Union type of all Literals
     * @category Linter
     * @experimental
     */
    export type Literal =
      | BigIntLiteral
      | BooleanLiteral
      | NullLiteral
      | NumberLiteral
      | RegExpLiteral
      | StringLiteral;

    /**
     * User named identifier inside JSX.
     * @category Linter
     * @experimental
     */
    export interface JSXIdentifier {
      type: "JSXIdentifier";
      range: Range;
      name: string;
      parent:
        | JSXNamespacedName
        | JSXOpeningElement
        | JSXAttribute
        | JSXClosingElement
        | JSXMemberExpression;
    }

    /**
     * Namespaced name in JSX
     * @category Linter
     * @experimental
     */
    export interface JSXNamespacedName {
      type: "JSXNamespacedName";
      range: Range;
      namespace: JSXIdentifier;
      name: JSXIdentifier;
      parent:
        | JSXOpeningElement
        | JSXAttribute
        | JSXClosingElement
        | JSXMemberExpression;
    }

    /**
     * Empty JSX expression.
     * @category Linter
     * @experimental
     */
    export interface JSXEmptyExpression {
      type: "JSXEmptyExpression";
      range: Range;
      parent: JSXAttribute | JSXElement | JSXFragment;
    }

    /**
     * A JSX element.
     * @category Linter
     * @experimental
     */
    export interface JSXElement {
      type: "JSXElement";
      range: Range;
      openingElement: JSXOpeningElement;
      closingElement: JSXClosingElement | null;
      children: JSXChild[];
      parent: Node;
    }

    /**
     * The opening tag of a JSXElement
     * @category Linter
     * @experimental
     */
    export interface JSXOpeningElement {
      type: "JSXOpeningElement";
      range: Range;
      selfClosing: boolean;
      name:
        | JSXIdentifier
        | JSXMemberExpression
        | JSXNamespacedName;
      attributes: Array<JSXAttribute | JSXSpreadAttribute>;
      typeArguments: TSTypeParameterInstantiation | undefined;
      parent: JSXElement;
    }

    /**
     * A JSX attribute
     * @category Linter
     * @experimental
     */
    export interface JSXAttribute {
      type: "JSXAttribute";
      range: Range;
      name: JSXIdentifier | JSXNamespacedName;
      value:
        | JSXElement
        | JSXExpressionContainer
        | Literal
        | null;
      parent: JSXOpeningElement;
    }

    /**
     * Spreads an object as JSX attributes.
     * @category Linter
     * @experimental
     */
    export interface JSXSpreadAttribute {
      type: "JSXSpreadAttribute";
      range: Range;
      argument: Expression;
      parent: JSXOpeningElement;
    }

    /**
     * The closing tag of a JSXElement. Only used when the element
     * is not self-closing.
     * @category Linter
     * @experimental
     */
    export interface JSXClosingElement {
      type: "JSXClosingElement";
      range: Range;
      name:
        | JSXIdentifier
        | JSXMemberExpression
        | JSXNamespacedName;
      parent: JSXElement;
    }

    /**
     * Usually a passthrough node to pass multiple sibling elements as
     * the JSX syntax requires one root element.
     * @category Linter
     * @experimental
     */
    export interface JSXFragment {
      type: "JSXFragment";
      range: Range;
      openingFragment: JSXOpeningFragment;
      closingFragment: JSXClosingFragment;
      children: JSXChild[];
      parent: Node;
    }

    /**
     * The opening tag of a JSXFragment.
     * @category Linter
     * @experimental
     */
    export interface JSXOpeningFragment {
      type: "JSXOpeningFragment";
      range: Range;
      parent: JSXFragment;
    }

    /**
     * The closing tag of a JSXFragment.
     * @category Linter
     * @experimental
     */
    export interface JSXClosingFragment {
      type: "JSXClosingFragment";
      range: Range;
      parent: JSXFragment;
    }

    /**
     * Inserts a normal JS expression into JSX.
     * @category Linter
     * @experimental
     */
    export interface JSXExpressionContainer {
      type: "JSXExpressionContainer";
      range: Range;
      expression: Expression | JSXEmptyExpression;
      parent: JSXAttribute | JSXElement | JSXFragment;
    }

    /**
     * Plain text in JSX.
     * @category Linter
     * @experimental
     */
    export interface JSXText {
      type: "JSXText";
      range: Range;
      raw: string;
      value: string;
      parent: JSXElement | JSXFragment;
    }

    /**
     * JSX member expression.
     * @category Linter
     * @experimental
     */
    export interface JSXMemberExpression {
      type: "JSXMemberExpression";
      range: Range;
      object:
        | JSXIdentifier
        | JSXMemberExpression
        | JSXNamespacedName;
      property: JSXIdentifier;
      parent: JSXOpeningElement | JSXClosingElement;
    }

    /**
     * Union type of all possible child nodes in JSX
     * @category Linter
     * @experimental
     */
    export type JSXChild =
      | JSXElement
      | JSXExpressionContainer
      | JSXFragment
      | JSXText;

    /**
     * @category Linter
     * @experimental
     */
    export interface TSModuleDeclaration {
      type: "TSModuleDeclaration";
      range: Range;
      declare: boolean;
      kind: "global" | "module" | "namespace";
      id: Identifier | Literal | TSQualifiedName;
      body: TSModuleBlock | undefined;
      parent:
        | ExportDefaultDeclaration
        | ExportNamedDeclaration
        | Program
        | StaticBlock
        | BlockStatement
        | WithStatement
        | LabeledStatement
        | IfStatement
        | SwitchCase
        | WhileStatement
        | DoWhileStatement
        | ForStatement
        | ForInStatement
        | ForOfStatement
        | TSModuleBlock;
    }

    /**
     * Body of a `TSModuleDeclaration`
     * @category Linter
     * @experimental
     */
    export interface TSModuleBlock {
      type: "TSModuleBlock";
      range: Range;
      body: Array<
        | ExportAllDeclaration
        | ExportDefaultDeclaration
        | ExportNamedDeclaration
        | ImportDeclaration
        | Statement
        | TSImportEqualsDeclaration
        | TSNamespaceExportDeclaration
      >;
      parent: TSModuleDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSClassImplements {
      type: "TSClassImplements";
      range: Range;
      expression: Expression;
      typeArguments: TSTypeParameterInstantiation | undefined;
      parent: ClassDeclaration | ClassExpression;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSAbstractMethodDefinition {
      type: "TSAbstractMethodDefinition";
      range: Range;
      computed: boolean;
      optional: boolean;
      override: boolean;
      static: boolean;
      accessibility: Accessibility | undefined;
      kind: "method";
      key: Expression | Identifier | NumberLiteral | StringLiteral;
      value: FunctionExpression | TSEmptyBodyFunctionExpression;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSAbstractPropertyDefinition {
      type: "TSAbstractPropertyDefinition";
      range: Range;
      computed: boolean;
      optional: boolean;
      override: boolean;
      static: boolean;
      definite: boolean;
      declare: boolean;
      readonly: boolean;
      accessibility: Accessibility | undefined;
      decorators: Decorator[];
      key:
        | Expression
        | PrivateIdentifier
        | Identifier
        | NumberLiteral
        | StringLiteral;
      typeAnnotation: TSTypeAnnotation | undefined;
      value: Expression | null;
      parent: ClassBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSEmptyBodyFunctionExpression {
      type: "TSEmptyBodyFunctionExpression";
      range: Range;
      declare: boolean;
      expression: boolean;
      async: boolean;
      generator: boolean;
      id: null;
      body: null;
      typeParameters: TSTypeParameterDeclaration | undefined;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      parent:
        | MethodDefinition
        | Property
        | TSAbstractMethodDefinition
        | TSParameterProperty;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSParameterProperty {
      type: "TSParameterProperty";
      range: Range;
      override: boolean;
      readonly: boolean;
      static: boolean;
      accessibility: Accessibility | undefined;
      decorators: Decorator[];
      parameter:
        | AssignmentPattern
        | ArrayPattern
        | ObjectPattern
        | Identifier
        | RestElement;
      parent:
        | ArrowFunctionExpression
        | FunctionDeclaration
        | FunctionExpression
        | TSDeclareFunction
        | TSEmptyBodyFunctionExpression;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSCallSignatureDeclaration {
      type: "TSCallSignatureDeclaration";
      range: Range;
      typeParameters: TSTypeParameterDeclaration | undefined;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      parent: TSInterfaceBody | TSTypeLiteral;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSPropertySignature {
      type: "TSPropertySignature";
      range: Range;
      computed: boolean;
      optional: boolean;
      readonly: boolean;
      static: boolean;
      key:
        | PrivateIdentifier
        | Expression
        | Identifier
        | NumberLiteral
        | StringLiteral;
      typeAnnotation: TSTypeAnnotation | undefined;
      parent: TSInterfaceBody | TSTypeLiteral;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSDeclareFunction {
      type: "TSDeclareFunction";
      range: Range;
      async: boolean;
      declare: boolean;
      generator: boolean;
      body: undefined;
      id: Identifier | null;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      typeParameters: TSTypeParameterDeclaration | undefined;
      parent: Node;
    }

    /**
     * ```ts
     * enum Foo { A, B };
     * ```
     * @category Linter
     * @experimental
     */
    export interface TSEnumDeclaration {
      type: "TSEnumDeclaration";
      range: Range;
      declare: boolean;
      const: boolean;
      id: Identifier;
      body: TSEnumBody;
      parent: Node;
    }

    /**
     * The body of a `TSEnumDeclaration`
     * @category Linter
     * @experimental
     */
    export interface TSEnumBody {
      type: "TSEnumBody";
      range: Range;
      members: TSEnumMember[];
      parent: TSEnumDeclaration;
    }

    /**
     * A member of a `TSEnumDeclaration`
     * @category Linter
     * @experimental
     */
    export interface TSEnumMember {
      type: "TSEnumMember";
      range: Range;
      id:
        | Identifier
        | NumberLiteral
        | StringLiteral;
      initializer: Expression | undefined;
      parent: TSEnumBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeAssertion {
      type: "TSTypeAssertion";
      range: Range;
      expression: Expression;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeParameterInstantiation {
      type: "TSTypeParameterInstantiation";
      range: Range;
      params: TypeNode[];
      parent:
        | ClassExpression
        | NewExpression
        | CallExpression
        | TaggedTemplateExpression
        | JSXOpeningElement
        | TSClassImplements
        | TSInstantiationExpression
        | TSInterfaceHeritage
        | TSTypeQuery
        | TSTypeReference
        | TSImportType;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeAliasDeclaration {
      type: "TSTypeAliasDeclaration";
      range: Range;
      declare: boolean;
      id: Identifier;
      typeParameters: TSTypeParameterDeclaration | undefined;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSSatisfiesExpression {
      type: "TSSatisfiesExpression";
      range: Range;
      expression: Expression;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSAsExpression {
      type: "TSAsExpression";
      range: Range;
      expression: Expression;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSInstantiationExpression {
      type: "TSInstantiationExpression";
      range: Range;
      expression: Expression;
      typeArguments: TSTypeParameterInstantiation;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNonNullExpression {
      type: "TSNonNullExpression";
      range: Range;
      expression: Expression;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSThisType {
      type: "TSThisType";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSInterfaceDeclaration {
      type: "TSInterfaceDeclaration";
      range: Range;
      declare: boolean;
      id: Identifier;
      extends: TSInterfaceHeritage[];
      typeParameters: TSTypeParameterDeclaration | undefined;
      body: TSInterfaceBody;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSInterfaceBody {
      type: "TSInterfaceBody";
      range: Range;
      body: Array<
        | TSCallSignatureDeclaration
        | TSConstructSignatureDeclaration
        | TSIndexSignature
        | TSMethodSignature
        | TSPropertySignature
      >;
      parent: TSInterfaceDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSConstructSignatureDeclaration {
      type: "TSConstructSignatureDeclaration";
      range: Range;
      typeParameters: TSTypeParameterDeclaration | undefined;
      params: Parameter[];
      returnType: TSTypeAnnotation;
      parent: TSInterfaceBody | TSTypeLiteral;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSMethodSignature {
      type: "TSMethodSignature";
      range: Range;
      computed: boolean;
      optional: boolean;
      readonly: boolean;
      static: boolean;
      kind: "get" | "set" | "method";
      key: Expression | Identifier | NumberLiteral | StringLiteral;
      returnType: TSTypeAnnotation | undefined;
      params: Parameter[];
      typeParameters: TSTypeParameterDeclaration | undefined;
      parent: TSInterfaceBody | TSTypeLiteral;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSInterfaceHeritage {
      type: "TSInterfaceHeritage";
      range: Range;
      expression: Expression;
      typeArguments: TSTypeParameterInstantiation | undefined;
      parent: TSInterfaceBody;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSIndexSignature {
      type: "TSIndexSignature";
      range: Range;
      readonly: boolean;
      static: boolean;
      parameters: Parameter[];
      typeAnnotation: TSTypeAnnotation | undefined;
      parent: ClassBody | TSInterfaceBody | TSTypeLiteral;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSUnionType {
      type: "TSUnionType";
      range: Range;
      types: TypeNode[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSIntersectionType {
      type: "TSIntersectionType";
      range: Range;
      types: TypeNode[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSInferType {
      type: "TSInferType";
      range: Range;
      typeParameter: TSTypeParameter;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeOperator {
      type: "TSTypeOperator";
      range: Range;
      operator: "keyof" | "readonly" | "unique";
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSIndexedAccessType {
      type: "TSIndexedAccessType";
      range: Range;
      indexType: TypeNode;
      objectType: TypeNode;
      parent: Node;
    }

    /**
     * ```ts
     * const a: any = null;
     * ```
     * @category Linter
     * @experimental
     */
    export interface TSAnyKeyword {
      type: "TSAnyKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSUnknownKeyword {
      type: "TSUnknownKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNumberKeyword {
      type: "TSNumberKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSObjectKeyword {
      type: "TSObjectKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSBooleanKeyword {
      type: "TSBooleanKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSBigIntKeyword {
      type: "TSBigIntKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSStringKeyword {
      type: "TSStringKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSSymbolKeyword {
      type: "TSSymbolKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSVoidKeyword {
      type: "TSVoidKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSUndefinedKeyword {
      type: "TSUndefinedKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNullKeyword {
      type: "TSNullKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNeverKeyword {
      type: "TSNeverKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSIntrinsicKeyword {
      type: "TSIntrinsicKeyword";
      range: Range;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSRestType {
      type: "TSRestType";
      range: Range;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSConditionalType {
      type: "TSConditionalType";
      range: Range;
      checkType: TypeNode;
      extendsType: TypeNode;
      trueType: TypeNode;
      falseType: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSMappedType {
      type: "TSMappedType";
      range: Range;
      readonly: boolean;
      optional: boolean;
      nameType: TypeNode | null;
      typeAnnotation: TypeNode | undefined;
      constraint: TypeNode;
      key: Identifier;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSLiteralType {
      type: "TSLiteralType";
      range: Range;
      literal: Literal | TemplateLiteral | UnaryExpression | UpdateExpression;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTemplateLiteralType {
      type: "TSTemplateLiteralType";
      range: Range;
      quasis: TemplateElement[];
      types: TypeNode[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeLiteral {
      type: "TSTypeLiteral";
      range: Range;
      members: Array<
        | TSCallSignatureDeclaration
        | TSConstructSignatureDeclaration
        | TSIndexSignature
        | TSMethodSignature
        | TSPropertySignature
      >;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSOptionalType {
      type: "TSOptionalType";
      range: Range;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeAnnotation {
      type: "TSTypeAnnotation";
      range: Range;
      typeAnnotation: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSArrayType {
      type: "TSArrayType";
      range: Range;
      elementType: TypeNode;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeQuery {
      type: "TSTypeQuery";
      range: Range;
      exprName: Identifier | ThisExpression | TSQualifiedName | TSImportType;
      typeArguments: TSTypeParameterInstantiation | undefined;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeReference {
      type: "TSTypeReference";
      range: Range;
      typeName: Identifier | ThisExpression | TSQualifiedName;
      typeArguments: TSTypeParameterInstantiation | undefined;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypePredicate {
      type: "TSTypePredicate";
      range: Range;
      asserts: boolean;
      parameterName: Identifier | TSThisType;
      typeAnnotation: TSTypeAnnotation | undefined;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTupleType {
      type: "TSTupleType";
      range: Range;
      elementTypes: TypeNode[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSNamedTupleMember {
      type: "TSNamedTupleMember";
      range: Range;
      label: Identifier;
      elementType: TypeNode;
      optional: boolean;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeParameterDeclaration {
      type: "TSTypeParameterDeclaration";
      range: Range;
      params: TSTypeParameter[];
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSTypeParameter {
      type: "TSTypeParameter";
      range: Range;
      in: boolean;
      out: boolean;
      const: boolean;
      name: Identifier;
      constraint: TypeNode | null;
      default: TypeNode | null;
      parent: TSInferType | TSMappedType | TSTypeParameterDeclaration;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSImportType {
      type: "TSImportType";
      range: Range;
      argument: TypeNode;
      qualifier: Identifier | ThisExpression | TSQualifiedName | null;
      typeArguments: TSTypeParameterInstantiation | null;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSExportAssignment {
      type: "TSExportAssignment";
      range: Range;
      expression: Expression;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSFunctionType {
      type: "TSFunctionType";
      range: Range;
      params: Parameter[];
      returnType: TSTypeAnnotation | undefined;
      typeParameters: TSTypeParameterDeclaration | undefined;
      parent: Node;
    }

    /**
     * @category Linter
     * @experimental
     */
    export interface TSQualifiedName {
      type: "TSQualifiedName";
      range: Range;
      left: Identifier | ThisExpression | TSQualifiedName;
      right: Identifier;
      parent: Node;
    }

    /**
     * Union type of all possible statement nodes
     * @category Linter
     * @experimental
     */
    export type Statement =
      | BlockStatement
      | BreakStatement
      | ClassDeclaration
      | ContinueStatement
      | DebuggerStatement
      | DoWhileStatement
      | ExportAllDeclaration
      | ExportDefaultDeclaration
      | ExportNamedDeclaration
      | ExpressionStatement
      | ForInStatement
      | ForOfStatement
      | ForStatement
      | FunctionDeclaration
      | IfStatement
      | ImportDeclaration
      | LabeledStatement
      | ReturnStatement
      | SwitchStatement
      | ThrowStatement
      | TryStatement
      | TSDeclareFunction
      | TSEnumDeclaration
      | TSExportAssignment
      | TSImportEqualsDeclaration
      | TSInterfaceDeclaration
      | TSModuleDeclaration
      | TSNamespaceExportDeclaration
      | TSTypeAliasDeclaration
      | VariableDeclaration
      | WhileStatement
      | WithStatement;

    /**
     * Union type of all possible expression nodes
     * @category Linter
     * @experimental
     */
    export type Expression =
      | ArrayExpression
      | ArrayPattern
      | ArrowFunctionExpression
      | AssignmentExpression
      | AwaitExpression
      | BinaryExpression
      | CallExpression
      | ChainExpression
      | ClassExpression
      | ConditionalExpression
      | FunctionExpression
      | Identifier
      | ImportExpression
      | JSXElement
      | JSXFragment
      | Literal
      | TemplateLiteral
      | LogicalExpression
      | MemberExpression
      | MetaProperty
      | NewExpression
      | ObjectExpression
      | ObjectPattern
      | SequenceExpression
      | Super
      | TaggedTemplateExpression
      | TemplateLiteral
      | ThisExpression
      | TSAsExpression
      | TSInstantiationExpression
      | TSNonNullExpression
      | TSSatisfiesExpression
      | TSTypeAssertion
      | UnaryExpression
      | UpdateExpression
      | YieldExpression;

    /**
     * Union type of all possible type nodes in TypeScript
     * @category Linter
     * @experimental
     */
    export type TypeNode =
      | TSAnyKeyword
      | TSArrayType
      | TSBigIntKeyword
      | TSBooleanKeyword
      | TSConditionalType
      | TSFunctionType
      | TSImportType
      | TSIndexedAccessType
      | TSInferType
      | TSIntersectionType
      | TSIntrinsicKeyword
      | TSLiteralType
      | TSMappedType
      | TSNamedTupleMember
      | TSNeverKeyword
      | TSNullKeyword
      | TSNumberKeyword
      | TSObjectKeyword
      | TSOptionalType
      | TSQualifiedName
      | TSRestType
      | TSStringKeyword
      | TSSymbolKeyword
      | TSTemplateLiteralType
      | TSThisType
      | TSTupleType
      | TSTypeLiteral
      | TSTypeOperator
      | TSTypePredicate
      | TSTypeQuery
      | TSTypeReference
      | TSUndefinedKeyword
      | TSUnionType
      | TSUnknownKeyword
      | TSVoidKeyword;

    /**
     * A single line comment
     * @category Linter
     * @experimental
     */
    export interface LineComment {
      type: "Line";
      range: Range;
      value: string;
    }

    /**
     * A potentially multi-line block comment
     * @category Linter
     * @experimental
     */
    export interface BlockComment {
      type: "Block";
      range: Range;
      value: string;
    }

    /**
     * Union type of all possible AST nodes
     * @category Linter
     * @experimental
     */
    export type Node =
      | Program
      | Expression
      | Statement
      | TypeNode
      | ImportSpecifier
      | ImportDefaultSpecifier
      | ImportNamespaceSpecifier
      | ImportAttribute
      | TSExternalModuleReference
      | ExportSpecifier
      | VariableDeclarator
      | Decorator
      | ClassBody
      | StaticBlock
      | PropertyDefinition
      | MethodDefinition
      | SwitchCase
      | CatchClause
      | TemplateElement
      | PrivateIdentifier
      | AssignmentPattern
      | RestElement
      | SpreadElement
      | Property
      | JSXIdentifier
      | JSXNamespacedName
      | JSXEmptyExpression
      | JSXOpeningElement
      | JSXAttribute
      | JSXSpreadAttribute
      | JSXClosingElement
      | JSXOpeningFragment
      | JSXClosingFragment
      | JSXExpressionContainer
      | JSXText
      | JSXMemberExpression
      | TSModuleBlock
      | TSClassImplements
      | TSAbstractMethodDefinition
      | TSAbstractPropertyDefinition
      | TSEmptyBodyFunctionExpression
      | TSCallSignatureDeclaration
      | TSPropertySignature
      | TSEnumBody
      | TSEnumMember
      | TSTypeParameterInstantiation
      | TSInterfaceBody
      | TSConstructSignatureDeclaration
      | TSMethodSignature
      | TSInterfaceHeritage
      | TSIndexSignature
      | TSTypeAnnotation
      | TSTypeParameterDeclaration
      | TSTypeParameter
      | LineComment
      | BlockComment;

    export {}; // only export exports
  }

  /**
   * The webgpu namespace provides additional APIs that the WebGPU specification
   * does not specify.
   *
   * @category GPU
   * @experimental
   */
  export namespace webgpu {
    /**
     * Starts a frame capture.
     *
     * This API is useful for debugging issues related to graphics, and makes
     * the captured data available to RenderDoc or XCode
     * (or other software for debugging frames)
     *
     * @category GPU
     * @experimental
     */
    export function deviceStartCapture(device: GPUDevice): void;
    /**
     * Stops a frame capture.
     *
     * This API is useful for debugging issues related to graphics, and makes
     * the captured data available to RenderDoc or XCode
     * (or other software for debugging frames)
     *
     * @category GPU
     * @experimental
     */
    export function deviceStopCapture(device: GPUDevice): void;

    export {}; // only export exports
  }

  export {}; // only export exports
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category Workers
 * @experimental
 */
interface WorkerOptions {
  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * Configure permissions options to change the level of access the worker will
   * have. By default it will inherit permissions. Note that the permissions
   * of a worker can't be extended beyond its parent's permissions reach.
   *
   * - `"inherit"` will use the default behavior and take the permissions of the
   *   thread the worker is created in
   * - `"none"` will have no permissions
   * - A list of routes can be provided that are relative to the file the worker
   *   is created in to limit the access of the worker (read/write permissions
   *   only)
   *
   * Example:
   *
   * ```ts
   * // mod.ts
   * const worker = new Worker(
   *   new URL("ende_worker.ts", import.meta.url).href, {
   *     type: "module",
   *     ende: {
   *       permissions: {
   *         read: true,
   *       },
   *     },
   *   }
   * );
   * ```
   */
  ende?: {
    /** Set to `"none"` to disable all the permissions in the worker. */
    permissions?: Ende.PermissionOptions;
  };
  /** **UNSTABLE**: New API, yet to be vetted.
   *
   * @deprecated Use {@linkcode WorkerOptions.ende} instead. When both are
   * given, `ende` is used.
   */
  deno?: {
    /** Set to `"none"` to disable all the permissions in the worker. */
    permissions?: Ende.PermissionOptions;
  };
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category WebSockets
 * @experimental
 */
interface WebSocketStreamOptions {
  protocols?: string[];
  signal?: AbortSignal;
  headers?: HeadersInit;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category WebSockets
 * @experimental
 */
interface WebSocketConnection {
  readable: ReadableStream<string | Uint8Array<ArrayBuffer>>;
  writable: WritableStream<string | Uint8Array<ArrayBufferLike>>;
  extensions: string;
  protocol: string;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category WebSockets
 * @experimental
 */
interface WebSocketCloseInfo {
  code?: number;
  reason?: string;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @tags allow-net
 * @category WebSockets
 * @experimental
 */
interface WebSocketStream {
  url: string;
  opened: Promise<WebSocketConnection>;
  closed: Promise<WebSocketCloseInfo>;
  close(closeInfo?: WebSocketCloseInfo): void;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @tags allow-net
 * @category WebSockets
 * @experimental
 */
declare var WebSocketStream: {
  readonly prototype: WebSocketStream;
  new (url: string, options?: WebSocketStreamOptions): WebSocketStream;
};

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @tags allow-net
 * @category WebSockets
 * @experimental
 */
interface WebSocketError extends DOMException {
  readonly closeCode: number;
  readonly reason: string;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @tags allow-net
 * @category WebSockets
 * @experimental
 */
declare var WebSocketError: {
  readonly prototype: WebSocketError;
  new (message?: string, init?: WebSocketCloseInfo): WebSocketError;
};

/**
 * @category Intl
 * @experimental
 */
declare namespace Intl {
  /**
   * Types that can be formatted using Intl.DateTimeFormat methods.
   *
   * This type defines what values can be passed to Intl.DateTimeFormat methods
   * for internationalized date and time formatting. It includes standard Date objects
   * and Temporal API date/time types.
   *
   * @example
   * ```ts
   * // Using with Date object
   * const date = new Date();
   * const formatter = new Intl.DateTimeFormat('en-US');
   * console.log(formatter.format(date));
   *
   * // Using with Temporal types (when available)
   * const instant = Temporal.Now.instant();
   * console.log(formatter.format(instant));
   * ```
   *
   * @category Intl
   * @experimental
   */
  export type Formattable =
    | Date
    | Temporal.Instant
    | Temporal.ZonedDateTime
    | Temporal.PlainDate
    | Temporal.PlainTime
    | Temporal.PlainDateTime
    | Temporal.PlainYearMonth
    | Temporal.PlainMonthDay;

  /**
   * Represents a part of a formatted date range produced by Intl.DateTimeFormat.formatRange().
   *
   * Each part has a type and value that describes its role within the formatted string.
   * The source property indicates whether the part comes from the start date, end date, or
   * is shared between them.
   *
   * @example
   * ```ts
   * const dtf = new Intl.DateTimeFormat('en', {
   *   dateStyle: 'long',
   *   timeStyle: 'short'
   * });
   * const parts = dtf.formatRangeToParts(
   *   new Date(2023, 0, 1, 12, 0),
   *   new Date(2023, 0, 3, 15, 30)
   * );
   * console.log(parts);
   * // Parts might include elements like:
   * // { type: 'month', value: 'January', source: 'startRange' }
   * // { type: 'day', value: '1', source: 'startRange' }
   * // { type: 'literal', value: ' - ', source: 'shared' }
   * // { type: 'day', value: '3', source: 'endRange' }
   * // ...
   * ```
   *
   * @category Intl
   * @experimental
   */
  export interface DateTimeFormatRangePart {
    /**
     * The type of date or time component this part represents.
     * Possible values: 'day', 'dayPeriod', 'era', 'fractionalSecond', 'hour',
     * 'literal', 'minute', 'month', 'relatedYear', 'second', 'timeZoneName',
     * 'weekday', 'year', etc.
     */
    type: string;

    /** The string value of this part. */
    value: string;

    /**
     * Indicates which date in the range this part comes from.
     * - 'startRange': The part is from the start date
     * - 'endRange': The part is from the end date
     * - 'shared': The part is shared between both dates (like separators)
     */
    source: "shared" | "startRange" | "endRange";
  }

  /**
   * @category Intl
   * @experimental
   */
  export interface DateTimeFormat {
    /**
     * Format a date into a string according to the locale and formatting
     * options of this `Intl.DateTimeFormat` object.
     *
     * @example
     * ```ts
     * const formatter = new Intl.DateTimeFormat('en-US', { dateStyle: 'full' });
     * const date = new Date(2023, 0, 1);
     * console.log(formatter.format(date)); // Output: "Sunday, January 1, 2023"
     * ```
     */
    format(date?: Formattable | number): string;

    /**
     * Allow locale-aware formatting of strings produced by
     * `Intl.DateTimeFormat` formatters.
     *
     * @example
     * ```ts
     * const formatter = new Intl.DateTimeFormat('en-US', { dateStyle: 'full' });
     * const date = new Date(2023, 0, 1);
     * console.log(formatter.format(date)); // Output: "Sunday, January 1, 2023"
     * ```
     */
    formatToParts(
      date?: Formattable | number,
    ): globalThis.Intl.DateTimeFormatPart[];

    /**
     * Format a date range in the most concise way based on the locale and
     * options provided when instantiating this `Intl.DateTimeFormat` object.
     *
     * @param startDate The start date of the range to format.
     * @param endDate The start date of the range to format. Must be the same
     * type as `startRange`.
     *
     * @example
     * ```ts
     * const formatter = new Intl.DateTimeFormat('en-US', { dateStyle: 'long' });
     * const startDate = new Date(2023, 0, 1);
     * const endDate = new Date(2023, 0, 5);
     * console.log(formatter.formatRange(startDate, endDate));
     * // Output: "January 1 – 5, 2023"
     * ```
     */
    formatRange<T extends Formattable>(startDate: T, endDate: T): string;
    formatRange(startDate: Date | number, endDate: Date | number): string;

    /**
     * Allow locale-aware formatting of tokens representing each part of the
     * formatted date range produced by `Intl.DateTimeFormat` formatters.
     *
     * @param startDate The start date of the range to format.
     * @param endDate The start date of the range to format. Must be the same
     * type as `startRange`.
     *
     * @example
     * ```ts
     * const formatter = new Intl.DateTimeFormat('en-US', { dateStyle: 'long' });
     * const startDate = new Date(2023, 0, 1);
     * const endDate = new Date(2023, 0, 5);
     * const parts = formatter.formatRangeToParts(startDate, endDate);
     * console.log(parts);
     * // Output might include:
     * // [
     * //   { type: 'month', value: 'January', source: 'startRange' },
     * //   { type: 'literal', value: ' ', source: 'shared' },
     * //   { type: 'day', value: '1', source: 'startRange' },
     * //   { type: 'literal', value: ' – ', source: 'shared' },
     * //   { type: 'day', value: '5', source: 'endRange' },
     * //   { type: 'literal', value: ', ', source: 'shared' },
     * //   { type: 'year', value: '2023', source: 'shared' }
     * // ]
     * ```
     */
    formatRangeToParts<T extends Formattable>(
      startDate: T,
      endDate: T,
    ): DateTimeFormatRangePart[];
    formatRangeToParts(
      startDate: Date | number,
      endDate: Date | number,
    ): DateTimeFormatRangePart[];
  }

  /**
   * @category Intl
   * @experimental
   */
  export interface DateTimeFormatOptions {
    // TODO: remove the props below after TS lib declarations are updated
    dayPeriod?: "narrow" | "short" | "long";
    dateStyle?: "full" | "long" | "medium" | "short";
    timeStyle?: "full" | "long" | "medium" | "short";
  }
}

/**
 * @category Platform
 * @experimental
 */
interface RegExpConstructor {
  /**
   * Returns a new string in which characters that are potentially special in a
   * regular expression pattern are replaced with escape sequences.
   * @param string The string to escape.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/RegExp/escape)
   */
  escape(string: string): string;
}

/**
 * @category Platform
 * @experimental
 */
interface Uint8Array {
  /**
   * Converts this `Uint8Array` object to a base64 string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/toBase64)
   */
  toBase64(options?: {
    alphabet?: "base64" | "base64url";
    omitPadding?: boolean;
  }): string;
  /**
   * Populates this `Uint8Array` object with data from a base64 string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/setFromBase64)
   */
  setFromBase64(string: string, options?: {
    alphabet?: "base64" | "base64url";
    lastChunkHandling?: "loose" | "strict" | "stop-before-partial";
  }): { read: number; written: number };
  /**
   * Converts this `Uint8Array` object to a hex string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/toHex)
   */
  toHex(): string;
  /**
   * Populates this `Uint8Array` object with data from a hex string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/setFromHex)
   */
  setFromHex(string: string): { read: number; written: number };
}

/**
 * @category Platform
 * @experimental
 */
interface Uint8ArrayConstructor {
  /**
   * Creates a new `Uint8Array` object from a base64 string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/fromBase64)
   */
  fromBase64(string: string, options?: {
    alphabet?: "base64" | "base64url";
    lastChunkHandling?: "loose" | "strict" | "stop-before-partial";
  }): Uint8Array<ArrayBuffer>;
  /**
   * Creates a new `Uint8Array` object from a hex string.
   *
   * [MDN](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array/fromHex)
   */
  fromHex(string: string): Uint8Array<ArrayBuffer>;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * A single CSS rule of a {@linkcode CSSStyleSheet}, as returned from its
 * `cssRules` property. Available only when the `--unstable-raw-imports` flag
 * is enabled.
 *
 * Note: `cssText` is the verbatim text of one top-level rule of the style
 * sheet; Ende does not implement a full CSS object model.
 *
 * @category Platform
 * @experimental
 */
interface CSSRule {
  readonly cssText: string;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category Platform
 * @experimental
 */
declare var CSSRule: {
  readonly prototype: CSSRule;
  new (): never;
};

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * A style sheet backing a CSS module script. This is what a
 * `import sheet from "./styles.css" with { type: "css" }` import evaluates
 * to. Available only when the `--unstable-raw-imports` flag is enabled.
 *
 * Ende has no DOM, so a sheet can't be adopted anywhere; the implementation
 * is backed by the raw CSS text.
 *
 * Note: `cssRules` returns a frozen array of {@linkcode CSSRule} instead of a
 * live `CSSRuleList`.
 *
 * @category Platform
 * @experimental
 */
interface CSSStyleSheet {
  readonly cssRules: readonly CSSRule[];
  replace(text: string): Promise<CSSStyleSheet>;
  replaceSync(text: string): void;
}

/** **UNSTABLE**: New API, yet to be vetted.
 *
 * @category Platform
 * @experimental
 */
declare var CSSStyleSheet: {
  readonly prototype: CSSStyleSheet;
  new (): CSSStyleSheet;
};
