# RustPBX: A Decoupled Architecture for AI Voice Agent Development

## System Architecture Overview

### Core Concepts of UA Mode

RustPBX's UserAgent (UA) mode introduces a decoupled architecture for AI voice agents, separating media processing from business logic through a WebSocket-based command/event interaction pattern.

This architectural approach allows developers to focus on AI logic implementation without requiring deep knowledge of audio processing complexities.

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                   Decoupled Architecture                       │
  │                                                                │
  │  ┌──────────────┐  WebSocket Commands  ┌──────────────────┐   │
  │  │   AI Agent    │ ──────────────────── │   RustPBX UA     │   │
  │  │              │ <──────────────────── │                  │   │
  │  └──────────────┘  WebSocket Events    └────────┬─────────┘   │
  │                                                 │              │
  └─────────────────────────────────────────────────┼──────────────┘
                          ┌─────────────────────────┼───────────┐
                          │                         │           │
               ┌──────────┴──────────┐    ┌────────┴────────┐
               │                     │    │                  │
  ┌────────────┴──────────────┐      │    │                  │
  │   Communication Layer     │      │    │                  │
  │                           │      │    │                  │
  │  ┌──────────────────────┐ │      │    │                  │
  │  │  SIP Proxy/Registry  │ │      │    │                  │
  │  └──────────────────────┘ │      │    │                  │
  │  ┌──────────────────────┐ │      │    │                  │
  │  │    WebRTC Peer       │ │      │    │                  │
  │  └──────────────────────┘ │      │    │                  │
  └───────────────────────────┘      │    │                  │
                                     │    │                  │
  ┌──────────────────────────────────┴────┴──────────────────┐
  │                 Media Processing Layer                    │
  │                                                          │
  │  ┌──────────────────────────┐                            │
  │  │  Audio Processing Engine │                            │
  │  └─────────┬────────────────┘                            │
  │            │                                             │
  │    ┌───────┼──────────┬──────────────┐                   │
  │    │       │          │              │                    │
  │  ┌─┴───────┴─┐  ┌────┴─────┐  ┌────┴──────┐  ┌───────────────┐
  │  │ASR Plugins│  │TTS Plugins│  │VAD Module │  │Noise Reduction│
  │  └───────────┘  └──────────┘  └───────────┘  └───────────────┘
  └──────────────────────────────────────────────────────────┘
```

### Key Architectural Features

1. **Separation of Concerns**: Complete isolation between AI logic and audio processing
2. **Flexible Extension**: Support for multiple ASR/TTS service providers
3. **Standard Protocols**: Built on SIP/WebRTC standards
4. **Real-time Interaction**: Low-latency bidirectional WebSocket communication


<div style="page-break-after: always;"></div>

## UA Mode Workflow

### Call Establishment and Media Negotiation

UA mode supports two operational patterns: registering with a SIP proxy server to receive calls, or initiating outbound calls. Media channels can utilize either SIP/RTP or WebRTC, providing significant flexibility.

```
  AI Agent          RustPBX UA          SIP Proxy        Remote Device
     │                  │                   │                  │
     │    Outbound Call Scenario            │                  │
     │                  │                   │                  │
     │  Invite Command  │                   │                  │
     │  (SIP/WebRTC)    │                   │                  │
     │ ────────────────>│                   │                  │
     │                  │  INVITE Request   │                  │
     │                  │ ────────────────> │                  │
     │                  │                   │  INVITE Forward  │
     │                  │                   │ ────────────────>│
     │                  │                   │  200 OK          │
     │                  │                   │  (SDP Answer)    │
     │                  │                   │ <────────────────│
     │                  │  200 OK Forward   │                  │
     │                  │ <──────────────── │                  │
     │  Answer Event    │                   │                  │
     │  (SDP)           │                   │                  │
     │ <────────────────│                   │                  │
     │                  │                   │                  │
     │  TrackStart Event│                   │                  │
     │ <────────────────│                   │                  │
     │  TTS Command     │                   │                  │
     │ ────────────────>│                   │                  │
     │                  │       Audio Stream (TTS)             │
     │                  │ ─────────────────────────────────── >│
     │                  │       Audio Stream (User Voice)      │
     │                  │ <─────────────────────────────────── │
     │  ASR Events      │                   │                  │
     │  (Speech Recog.) │                   │                  │
     │ <────────────────│                   │                  │
     │  Response Commands                   │                  │
     │ ────────────────>│                   │                  │
     │                  │                   │                  │
```

### Key Component Interactions

The UA manages the complete call lifecycle, including media streams, event processing, and command dispatch.


<div style="page-break-after: always;"></div>

## Command/Event Interaction Mechanism

### WebSocket Protocol for Decoupled Architecture

RustPBX implements a decoupled architecture where AI Agents send commands via WebSocket to control UA behavior, while UA provides status feedback through events.

```
  ┌─────────────────────────────┐         ┌─────────────────────────────┐
  │      AI Agent Process       │         │     RustPBX UA Process      │
  │                             │         │                             │
  │  ┌───────────────────────┐  │  TTS    │  ┌───────────────────────┐  │
  │  │    Business Logic     │──┼─Command──┼─>│    Command Handler    │  │
  │  └───────────────────────┘  │         │  └───────────┬───────────┘  │
  │  ┌───────────────────────┐  │  Play   │              │              │
  │  │    LLM Integration    │  │ Command │              v              │
  │  └───────────────────────┘  │─────────┼─>┌───────────────────────┐  │
  │  ┌───────────────────────┐  │  Hangup │  │     Media Engine      │  │
  │  │    Decision Engine    │  │ Command │  └───────────┬───────────┘  │
  │  └───────────────────────┘  │─────────┼─>            │              │
  │              ^               │         │              v              │
  │              │               │         │  ┌───────────────────────┐  │
  │              │  ASR Events   │         │  │    Event Generator    │  │
  │              ├───────────────┼─────────┼──│                       │  │
  │              │  DTMF Events  │         │  └───────────────────────┘  │
  │              ├───────────────┼─────────┼──                          │
  │              │  TrackStart/  │         │                             │
  │              │  End Events   │         │                             │
  │              └───────────────┼─────────┼──                          │
  └─────────────────────────────┘         └─────────────────────────────┘
```

### Core Command Types

```rust
enum Command {
    Tts { text, speaker, auto_hangup, streaming, ... },
    Play { url, auto_hangup, wait_input_timeout },
    Hangup { reason, initiator },
    Refer { caller, callee, options },
    Mute/Unmute { track_id },
    Pause/Resume,
    Interrupt,
}
```

### Primary Event Types

- **ASR Events**: Real-time speech recognition results
- **DTMF Events**: Touch-tone detection
- **Track Events**: Media track lifecycle changes
- **Silence Events**: Voice activity detection


<div style="page-break-after: always;"></div>

## Plugin-based ASR/TTS Architecture

### Multi-Provider Support

RustPBX employs a plugin architecture supporting various ASR/TTS service providers, including cloud services like Alibaba Cloud and Tencent Cloud, enabling developers to choose the most suitable provider or implement custom solutions.

```
                    ┌─────────────────┐
                    │   MediaStream   │
                    └────┬───────┬────┘
                         │       │
                         v       v
               ┌─────────────┐ ┌───────────┐
               │ASR Processor│ │ TTS Engine│
               └──┬──┬──┬────┘ └──┬──┬──┬──┘
                  │  │  │         │  │  │
     ┌────────────┘  │  └───┐    │  │  └──────────────┐
     │               │      │    │  │                  │
     v               v      v    v  v                  v
  ┌──────────────────────────┐  ┌──────────────────────────┐
  │       ASR Plugins        │  │       TTS Plugins        │
  │                          │  │                          │
  │  ┌────────────────────┐  │  │  ┌────────────────────┐  │
  │  │ Alibaba Cloud ASR  │  │  │  │ Alibaba Cloud TTS  │  │
  │  └────────────────────┘  │  │  └────────────────────┘  │
  │  ┌────────────────────┐  │  │  ┌────────────────────┐  │
  │  │ Tencent Cloud ASR  │  │  │  │ Tencent Cloud TTS  │  │
  │  └────────────────────┘  │  │  └────────────────────┘  │
  │  ┌────────────────────┐  │  │  ┌────────────────────┐  │
  │  │    Custom ASR      │  │  │  │    Custom TTS      │  │
  │  └────────────────────┘  │  │  └────────────────────┘  │
  └──────────────────────────┘  └──────────────────────────┘
                  ^                        ^
                  │                        │
                  └────────┬───────────────┘
                           │
                    ┌──────┴──────┐
                    │Configuration│
                    └─────────────┘
```

### Extension Benefits

- **Unified Interface**: All ASR/TTS plugins implement consistent traits
- **Hot-swappable**: Runtime provider switching capability
- **Configuration-driven**: Easy switching through configuration files
- **Error Handling**: Built-in retry and fallback mechanisms


<div style="page-break-after: always;"></div>

## Built-in Audio Processing Features

### Advanced Audio Processing Capabilities

RustPBX includes comprehensive audio processing features such as VAD (Voice Activity Detection), noise reduction, and background noise enhancement, significantly improving voice interaction quality.

```
  ┌──────────────────┐
  │ Raw Audio Stream │
  └────────┬─────────┘
           v
  ┌──────────────────┐
  │  VAD Detection   │
  └────────┬─────────┘
           v
     ┌───────────┐
     │  Voice    │
     │ Detected? │
     └──┬─────┬──┘
   Yes  │     │  No
        v     v
  ┌───────────────┐   ┌─────────────────────┐
  │Noise Reduction│   │ Silence Processing  │
  └───────┬───────┘   └──────────┬──────────┘
          v                      v
  ┌───────────────┐   ┌─────────────────────────┐
  │Audio Enhance- │   │Background Noise Detection│
  │ment           │   └──────────┬──────────────┘
  └───────┬───────┘              v
          v              ┌───────────────┐
  ┌───────────────┐      │Silence Events │
  │ASR Processing │      └───────┬───────┘
  └───────┬───────┘              v
          v              ┌───────────────┐
  ┌───────────────────┐  │Silence Events │
  │Recognition Results│  └───────────────┘
  └───────────────────┘

  ┌──────────────────────────────────────────┐
  │        Audio Processing Pipeline         │
  │                                          │
  │  ┌───────────────┐ ┌──────────────────┐  │
  │  │ Jitter Buffer │ │Echo Cancellation │  │
  │  └───────────────┘ └──────────────────┘  │
  │  ┌──────────────────────┐                │
  │  │Automatic Gain Control│                │
  │  └──────────────────────┘                │
  └──────────────────────────────────────────┘
```

### Advanced TTS/Play Features

RustPBX implements various practical features for TTS and audio playback:

```rust
TtsCommand {
    streaming: Some(true),         // Enable streaming output
    end_of_stream: Some(false),    // Mark stream end
    auto_hangup: Some(true),       // Auto-hangup after playback
    wait_input_timeout: Some(30),  // Input timeout in seconds
}
```

### Feature Highlights

1. **Streaming TTS**: Real-time generation and playback for reduced latency
2. **Auto-hangup**: Automatic call termination after playback completion
3. **Input Waiting**: Post-playback waiting for voice or DTMF input
4. **Interruption Support**: User-initiated playback interruption
5. **Background Music**: Hold music during wait periods


<div style="page-break-after: always;"></div>

## Architectural Comparison: RustPBX UA Mode vs Alternative Approaches

### Design Philosophy Differences

RustPBX UA mode represents a decoupled architectural approach for AI voice agents. While some frameworks utilize monolithic architectures with all components in a single process, RustPBX separates AI logic from media processing entirely.

```
  ┌──────────────────────────┐    ┌────────────────────────────────────┐
  │  Monolithic Architecture │    │  RustPBX Decoupled Architecture   │
  │                          │    │                                    │
  │  ┌────────────────────┐  │    │  ┌──────────────────────────────┐  │
  │  │     AI Logic       │  │    │  │     AI Agent Process         │  │
  │  └─────────┬──────────┘  │    │  └──────────────┬───────────────┘  │
  │            v             │    │                 v                  │
  │  ┌────────────────────┐  │    │  ┌──────────────────────────────┐  │
  │  │  Audio Processing  │  │    │  │    WebSocket Interface       │  │
  │  └─────────┬──────────┘  │    │  └──────────────┬───────────────┘  │
  │            v             │    │                 v                  │
  │  ┌────────────────────┐  │    │  ┌──────────────────────────────┐  │
  │  │  Protocol Stack    │  │    │  │    RustPBX UA Process        │  │
  │  └─────────┬──────────┘  │    │  └──────────┬──────┬────────────┘  │
  │            v             │    │             v      v              │
  │  ┌────────────────────┐  │    │  ┌──────────────┐ ┌────────────┐  │
  │  │   Media Codecs     │  │    │  │ Built-in     │ │ Built-in   │  │
  │  └─────────┬──────────┘  │    │  │ Audio        │ │ Protocol   │  │
  │            v             │    │  │ Processing   │ │ Stack      │  │
  │  ┌────────────────────┐  │    │  └──────────────┘ └────────────┘  │
  │  │     ASR/TTS        │  │    │                                    │
  │  └────────────────────┘  │    │                                    │
  └──────────────────────────┘    └────────────────────────────────────┘
```

### Development Approach Comparison

```
  ┌──────────────────────────────────────────────────────────────┐
  │                   Traditional Approach                       │
  │                                                              │
  │  ┌────────────────────┐    ┌──────────────────────┐          │
  │  │Single Language     │───>│Integrated Components │          │
  │  │Stack               │    └──────────┬───────────┘          │
  │  └────────────────────┘               v                      │
  │                            ┌──────────────────────┐          │
  │                            │Synchronous Processing│          │
  │                            └──────────┬───────────┘          │
  │                                       v                      │
  │                            ┌──────────────────────┐          │
  │                            │ Framework Coupling   │          │
  │                            └──────────────────────┘          │
  └──────────────────────────────────────────────────────────────┘

  ┌──────────────────────────────────────────────────────────────┐
  │                     RustPBX Approach                         │
  │                                                              │
  │  ┌────────────────────┐    ┌──────────────────────┐          │
  │  │Multi-language      │───>│ Process Separation   │          │
  │  │Support             │    └──────────┬───────────┘          │
  │  └────────────────────┘               v                      │
  │                            ┌──────────────────────────┐      │
  │                            │Event-driven Architecture │      │
  │                            └──────────┬───────────────┘      │
  │                                       v                      │
  │                            ┌──────────────────────┐          │
  │                            │ Technology Freedom   │          │
  │                            └──────────────────────┘          │
  └──────────────────────────────────────────────────────────────┘
```

### Key Architectural Differences

| Aspect                 | Monolithic Approach                    | RustPBX UA Mode                       |
| ---------------------- | -------------------------------------- | ------------------------------------- |
| **Architecture**       | Integrated components                  | Decoupled AI and media                |
| **Language Support**   | Framework-specific                     | Any language (Python/JS/Go/Java/etc.) |
| **Deployment**         | Single process                         | Distributed, independently scalable   |
| **Learning Curve**     | Requires audio processing knowledge    | Focus on AI logic only                |
| **Debugging**          | Coupled AI and media issues            | Isolated problem domains              |
| **Extensibility**      | Framework-bound extensions             | Plugin-based, hot-swappable           |
| **Technical Debt**     | Framework upgrades affect entire stack | Independent AI and media evolution    |
| **Team Collaboration** | Requires full-stack developers         | Enables specialized team roles        |

### Use Case Considerations

**Monolithic architectures may suit:**
- Rapid prototyping and validation
- Small-scale or personal projects
- Strong framework ecosystem dependencies
- Simple voice interaction requirements

**RustPBX UA mode excels for:**
- Enterprise applications and production environments
- High-scalability system requirements
- Multi-team collaborative development
- Complex voice interaction scenarios
- Performance and stability-critical applications

### Performance and Maintainability

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                  Performance Characteristics                    │
  │                                                                 │
  │   Traditional                    RustPBX                        │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │Traditional Performance│       │  RustPBX Performance    │    │
  │  └──────────┬───────────┘       └────────────┬─────────────┘    │
  │             v                                v                  │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │Single Process Limits │       │Multi-process Parallelism │    │
  │  └──────────┬───────────┘       └────────────┬─────────────┘    │
  │             v                                v                  │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │ Runtime Constraints  │       │High-performance Runtime  │    │
  │  └──────────────────────┘       └──────────────────────────┘    │
  └─────────────────────────────────────────────────────────────────┘

  ┌─────────────────────────────────────────────────────────────────┐
  │                   Maintainability Aspects                       │
  │                                                                 │
  │   Traditional                    RustPBX                        │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │Traditional Maintenance│       │  RustPBX Maintenance    │    │
  │  └──────────┬───────────┘       └────────────┬─────────────┘    │
  │             v                                v                  │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │   High Coupling      │       │  Modular Decoupling      │    │
  │  └──────────┬───────────┘       └────────────┬─────────────┘    │
  │             v                                v                  │
  │  ┌──────────────────────┐       ┌──────────────────────────┐    │
  │  │  Complex Debugging   │       │  Problem Isolation       │    │
  │  └──────────────────────┘       └──────────────────────────┘    │
  └─────────────────────────────────────────────────────────────────┘
```


<div style="page-break-after: always;"></div>

## Voice Agent Development Benefits

### Development Productivity Analysis

RustPBX's UA mode provides significant productivity benefits for Voice Agent development, enabling developers to concentrate on AI logic without handling complex audio and communication protocols.

```
  ┌─────────────────────────────┐    ┌─────────────────────────────────┐
  │ Traditional Development     │    │       RustPBX UA Mode           │
  │ Model                       │    │                                 │
  │                             │    │  ┌────────────────────────────┐ │
  │  ┌───────────────────────┐  │    │  │       AI Logic            │ │
  │  │      AI Logic         │  │    │  └─────────────┬──────────────┘ │
  │  └ ─ ─ ─ ─ ┬ ─ ─ ─ ─ ─ ┘  │    │               v               │
  │             v               │    │  ┌────────────────────────────┐ │
  │  ┌───────────────────────┐  │    │  │   WebSocket Interface     │ │
  │  │  Audio Processing     │  │    │  └─────────────┬──────────────┘ │
  │  └ ─ ─ ─ ─ ┬ ─ ─ ─ ─ ─ ┘  │    │               v               │
  │             v               │    │  ┌────────────────────────────┐ │
  │  ┌───────────────────────┐  │    │  │      RustPBX UA           │ │
  │  │  Protocol Handling    │  │    │  └──────────┬──────┬──────────┘ │
  │  └ ─ ─ ─ ─ ┬ ─ ─ ─ ─ ─ ┘  │    │            v      v           │
  │             v               │    │  ┌────────────┐ ┌───────────┐  │
  │  ┌───────────────────────┐  │    │  │ Built-in   │ │ Built-in  │  │
  │  │    Media Codecs       │  │    │  │ Audio      │ │ Protocol  │  │
  │  └───────────────────────┘  │    │  │ Processing │ │ Stack     │  │
  │                             │    │  └────────────┘ └───────────┘  │
  └─────────────────────────────┘    └─────────────────────────────────┘
                                                  ^
                                                  │
                                     ┌────────────┴────────────┐
                                     │  Developer Focus Area   │
                                     └─────────────────────────┘
```

### Technology Stack Freedom

Developers can utilize any programming language and AI framework:

- **Python**: LangChain, OpenAI SDK, FastAPI
- **JavaScript**: Node.js, OpenAI API, Express
- **Java**: Spring Boot, AI frameworks
- **Go**: Custom AI logic, high-performance services
- **Rust**: Local AI inference, system-level optimization

### Deployment Architecture Flexibility

```
  ┌───────────────────────┐          ┌───────────────────────┐
  │   AI Agent Server     │          │    RustPBX Server     │
  │                       │WebSocket │                       │
  │  ┌─────────────────┐  │         │  ┌─────────────────┐  │
  │  │ Business Logic  │──┼─────────┼─>│   UA Instance   │  │
  │  └─────────────────┘  │         │  └────────┬────────┘  │
  │  ┌─────────────────┐  │         │  ┌────────┴────────┐  │
  │  │  LLM Services   │  │         │  │Media Processing │  │
  │  └─────────────────┘  │         │  └─────────────────┘  │
  │  ┌─────────────────┐  │         │  ┌─────────────────┐  │
  │  │ Knowledge Base  │  │         │  │ Protocol Stack  │  │
  │  └─────────────────┘  │         │  └─────────────────┘  │
  └───────────────────────┘          └───────────┬───────────┘
                                          │           │
                                     API  │           │  SIP
                                          v           v
                                  ┌───────────────────────────┐
                                  │    External Services      │
                                  │                           │
                                  │  ┌───────────────────┐    │
                                  │  │ ASR/TTS Services  │    │
                                  │  └───────────────────┘    │
                                  │  ┌───────────────────┐    │
                                  │  │   SIP Carriers    │    │
                                  │  └───────────────────┘    │
                                  └───────────────────────────┘
```

### Development Advantages Summary

1. **Reduced Learning Curve**: No audio processing or SIP protocol expertise required
2. **Accelerated Development**: Focus on business logic enables rapid iteration
3. **Enhanced Testability**: WebSocket interface facilitates unit testing
4. **Horizontal Scalability**: Distributed deployment with support for WebRTC/SIP/WebSocket
5. **Technology Agnostic**: No constraints on AI development technology choices
6. **Production Ready**: Built-in monitoring, logging, and error handling


<div style="page-break-after: always;"></div>

## Conclusion

RustPBX's UA mode delivers a robust and flexible platform for AI Voice Agent development through its decoupled architecture, WebSocket interaction mechanism, plugin-based design, and comprehensive audio processing capabilities. This approach enables developers to focus on AI logic implementation while delegating complex audio processing and communication protocols to RustPBX, significantly reducing development barriers and enhancing productivity.

The architecture's emphasis on separation of concerns, technology freedom, and operational flexibility makes it particularly well-suited for enterprise applications and production environments where scalability, maintainability, and team collaboration are paramount considerations.
