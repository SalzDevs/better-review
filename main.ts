import {Effect} from "effect"

const getGitDiff = (): Effect.Effect<string, Error, never> =>
  Effect.async((resolve) => {
    const command = new Deno.Command("git",{
      args: ["diff"],
      stdout: "piped",
      stderr: "piped",
    })

    command.output().then((output)=> {
      const decoder = new TextDecoder()
      if (output.code !== 0) {
        const error = decoder.decode(output.stderr)
        resolve(Effect.fail(new Error(`Error executing git diff: ${error.message}`)))
        return
      }
      const stdout = decoder.decode(output.stdout)
      resolve(Effect.succeed(stdout))
    })
  })


Effect.runPromise(getGitDiff())
  .then((diff) => {
    console.log("Git Diff Output:")
    console.log(diff)
  })
  .catch((error) => {
    console.error("Error:", error.message)
  })


