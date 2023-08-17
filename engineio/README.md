# Reading List:
https://websockets.readthedocs.io/en/stable/howto/sansio.html


# Engine IO 

- State Machine where each input transitions internal state and puts bytes into a buffer to send back
- Error should be returned as Results, if transition is illegal 



- What does the Engine do? 
    - Handshakes 
    - upgrades
    - config set
    - framing
    - heartbeats


- Converting IO into events 
    - Both TCP level AND HTTP level must be fed into the engine? 
    - The input must HTTP reqs, and or WS frames 
    


#Scenarios 

## 1. New Connection
1. New Stream
2. Parse HTTP - TODO 
3. Get Sid
4. No Sid, create new Connection
5. Send connect data to Engine
6. return response
7. translate back to HTTP - TODO

TODO: Unit test SID stuff 

# 2. GET events - nothing
1. Parse HTTP
2. get Sid
3. Find Correct Engine - TODO
4. Poll Output, receive nothing ... TODO
5. DONT send response
6. Wait until next ping pong
7. OR next event... how ?
8. writer must notify after adding to buffer - seperate IO 
    - Mutex release? 
    - Channel ?


WE should start writing the next level of library - the full fat RUST framework for SIO 
so we can try all this! 

- Create Socketio module
- setup artix 
- create sample
- try send event


